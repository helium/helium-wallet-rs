use crate::{
    keypair::{Network, PublicKey},
    mnemonic,
    result::{bail, Error, Result},
    traits::{TxnFeeConfig, B64},
    wallet::Wallet,
};
pub use helium_api::{
    models::{transactions::PendingTxnStatus, Hnt, Hst, Usd},
    Client,
};
pub use helium_proto::*;
pub use serde_json::json;

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};
pub use structopt::{clap::arg_enum, StructOpt};

pub mod balance;
pub mod burn;
pub mod commit;
pub mod create;
pub mod hotspots;
pub mod htlc;
pub mod info;
pub mod multisig;
pub mod oracle;
pub mod oui;
pub mod pay;
pub mod request;
pub mod securities;
pub mod sign;
pub mod upgrade;
pub mod validators;
pub mod vars;
pub mod verify;

arg_enum! {
    #[derive(Debug)]
    pub enum OutputFormat {
        Table,
        Json,
    }
}

/// Common options for most wallet commands
#[derive(Debug, StructOpt)]
pub struct Opts {
    /// File(s) to use
    #[structopt(
        short = "f",
        long = "file",
        number_of_values(1),
        default_value = "wallet.key"
    )]
    files: Vec<PathBuf>,

    /// Output format to use
    #[structopt(long = "format",
                possible_values = &["table", "json"],
                case_insensitive = true,
                default_value = "table")]
    format: OutputFormat,
}

#[derive(Debug, Clone)]
pub struct Transaction(BlockchainTxn);

impl std::str::FromStr for Transaction {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(Self(BlockchainTxn::from_b64(s)?))
    }
}

fn load_wallet(files: Vec<PathBuf>) -> Result<Wallet> {
    let mut files_iter = files.iter();
    let mut first_wallet = match files_iter.next() {
        Some(path) => {
            let mut reader = fs::File::open(path)?;
            Wallet::read(&mut reader)?
        }
        None => bail!("At least one wallet file expected"),
    };

    for path in files_iter {
        let mut reader = fs::File::open(path)?;
        let w = Wallet::read(&mut reader)?;
        first_wallet.absorb_shard(&w)?;
    }

    Ok(first_wallet)
}

fn get_password(confirm: bool) -> std::io::Result<String> {
    match env::var("HELIUM_WALLET_PASSWORD") {
        Ok(str) => Ok(str),
        _ => {
            use dialoguer::Password;
            let mut builder = Password::new();
            builder.with_prompt("Password");
            if confirm {
                builder.with_confirmation("Confirm password", "Passwords do not match");
            };
            builder.interact()
        }
    }
}

const DEFAULT_TESTNET_BASE_URL: &str = "https://testnet-api.helium.wtf/v1";

fn api_url(network: Network) -> String {
    match network {
        Network::MainNet => {
            env::var("HELIUM_API_URL").unwrap_or_else(|_| helium_api::DEFAULT_BASE_URL.to_string())
        }
        Network::TestNet => env::var("HELIUM_TESTNET_API_URL")
            .unwrap_or_else(|_| DEFAULT_TESTNET_BASE_URL.to_string()),
    }
}

pub(crate) static USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

fn new_client(base_url: String) -> Client {
    Client::new_with_base_url(base_url, USER_AGENT)
}

fn read_txn(txn: &Option<Transaction>) -> Result<BlockchainTxn> {
    match txn {
        Some(txn) => Ok(txn.0.clone()),
        None => {
            let mut buffer = String::new();
            io::stdin().read_line(&mut buffer)?;
            Ok(buffer.trim().parse::<Transaction>()?.0)
        }
    }
}

fn collect_addresses(files: Vec<PathBuf>, mut addresses: Vec<PublicKey>) -> Result<Vec<PublicKey>> {
    // Any given addresses override _all_ the file parameters
    if addresses.is_empty() {
        for file in files {
            let mut reader = fs::File::open(&file)?;
            let enc_wallet = Wallet::read(&mut reader)?;
            addresses.push(enc_wallet.public_key);
        }
    }
    Ok(addresses)
}

fn get_seed_words(seed_type: &mnemonic::SeedType) -> Result<Vec<String>> {
    use dialoguer::Input;
    let split_str = |s: &String| s.split_whitespace().map(|w| w.to_string()).collect();
    let word_string = Input::<String>::new()
        .with_prompt("Space separated seed words")
        .validate_with(|v: &String| {
            let word_list = split_str(v);
            match mnemonic::mnemonic_to_entropy(word_list, seed_type) {
                Ok(_) => Ok(()),
                Err(err) => Err(err),
            }
        })
        .interact()?;
    Ok(word_string
        .split_whitespace()
        .map(|w| w.to_string())
        .collect())
}

pub fn get_payer(staking_address: PublicKey, payer: &Option<String>) -> Result<Option<PublicKey>> {
    match payer {
        Some(s) if s == "staking" => Ok(Some(staking_address)),
        Some(s) => {
            let address = s.parse()?;
            Ok(Some(address))
        }
        None => Ok(None),
    }
}

pub async fn get_txn_fees(client: &Client) -> Result<TxnFeeConfig> {
    let vars = helium_api::vars::get(client).await?;
    if vars.contains_key("txn_fees") {
        match vars["txn_fees"].as_bool() {
            Some(true) => {
                let config: TxnFeeConfig = serde_json::from_value(serde_json::Value::Object(vars))?;
                Ok(config)
            }
            _ => Ok(TxnFeeConfig::legacy()),
        }
    } else {
        Ok(TxnFeeConfig::legacy())
    }
}

pub fn open_output_file(filename: &Path, create: bool) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .create_new(create)
        .truncate(true)
        .open(filename)
}

pub fn get_file_extension(filename: &Path) -> String {
    use std::ffi::OsStr;
    filename
        .extension()
        .unwrap_or_else(|| OsStr::new(""))
        .to_str()
        .unwrap()
        .to_string()
}

pub fn print_footer(status: &Option<PendingTxnStatus>) -> Result {
    if status.is_none() {
        println!("\nPreview mode: use --commit to submit the transaction to the network");
    };
    Ok(())
}

pub fn print_json<T: ?Sized + serde::Serialize>(value: &T) -> Result {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn print_table(table: &prettytable::Table) -> Result {
    table.printstd();
    Ok(())
}

pub fn status_str(status: &Option<PendingTxnStatus>) -> &str {
    status.as_ref().map_or("none", |s| &s.hash)
}

pub fn status_json(status: &Option<PendingTxnStatus>) -> serde_json::Value {
    status.as_ref().map_or(json!(null), |s| json!(s.hash))
}

pub async fn maybe_submit_txn(
    commit: bool,
    client: &Client,
    txn: &BlockchainTxn,
) -> Result<Option<PendingTxnStatus>> {
    if commit {
        let status = submit_txn(client, txn).await?;
        Ok(Some(status))
    } else {
        Ok(None)
    }
}

pub async fn submit_txn(client: &Client, txn: &BlockchainTxn) -> Result<PendingTxnStatus> {
    let mut data = vec![];
    txn.encode(&mut data)?;
    helium_api::pending_transactions::submit(client, &data)
        .await
        .map_err(|e| e.into())
}
