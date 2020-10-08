use crate::{
    keypair::PubKeyBin,
    mnemonic,
    result::Result,
    traits::{TxnFeeConfig, B58},
    wallet::Wallet,
};
use helium_api::{Client, PendingTxnStatus};
use std::{env, fs, io, path::PathBuf};
use structopt::{clap::arg_enum, StructOpt};

pub mod balance;
pub mod burn;
pub mod create;
pub mod hotspots;
pub mod htlc;
pub mod info;
pub mod multisig;
pub mod onboard;
pub mod oracle;
pub mod oui;
pub mod pay;
pub mod request;
pub mod securities;
pub mod upgrade;
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

    /// Output formwat to use
    #[structopt(long = "format",
                possible_values = &["table", "json"],
                case_insensitive = true,
                default_value = "table")]
    format: OutputFormat,
}

fn load_wallet(files: Vec<PathBuf>) -> Result<Wallet> {
    let mut files_iter = files.iter();
    let mut first_wallet = match files_iter.next() {
        Some(path) => {
            let mut reader = fs::File::open(path)?;
            Wallet::read(&mut reader)?
        }
        None => return Err("At least one wallet file expected".into()),
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

fn api_url() -> String {
    env::var("HELIUM_API_URL").unwrap_or_else(|_| helium_api::DEFAULT_BASE_URL.to_string())
}

fn collect_addresses(files: Vec<PathBuf>, mut addresses: Vec<String>) -> Result<Vec<String>> {
    // Any given addresses override _all_ the file parameters
    if addresses.is_empty() {
        for file in files {
            let mut reader = fs::File::open(&file)?;
            let enc_wallet = Wallet::read(&mut reader)?;
            addresses.push(enc_wallet.address()?);
        }
    }
    Ok(addresses)
}

fn get_seed_words() -> Result<Vec<String>> {
    use dialoguer::Input;
    let split_str = |s: String| s.split_whitespace().map(|w| w.to_string()).collect();
    let word_string = Input::<String>::new()
        .with_prompt("Seed Words")
        .validate_with(move |v: &str| {
            let word_list = split_str(v.to_string());
            match mnemonic::mnemonic_to_entropy(word_list) {
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

pub fn get_payer(staking_address: PubKeyBin, payer: &Option<String>) -> Result<Option<PubKeyBin>> {
    match payer {
        Some(s) if s == "staking" => Ok(Some(staking_address)),
        Some(s) => {
            let address = PubKeyBin::from_b58(&s)?;
            Ok(Some(address))
        }
        None => Ok(None),
    }
}

pub fn get_txn_fees(client: &Client) -> Result<TxnFeeConfig> {
    let vars = client.get_vars()?;
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

pub fn open_output_file(filename: &PathBuf, create: bool) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .create_new(create)
        .open(filename)
}

pub fn get_file_extension(filename: &PathBuf) -> String {
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
