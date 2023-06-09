use crate::{
    b64,
    client::Client,
    result::{bail, Error, Result},
    wallet::Wallet,
};
pub use helium_proto::*;
pub use serde_json::json;

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use anchor_client::{solana_client, solana_sdk};

pub mod balance;
pub mod create;
pub mod dc;
pub mod export;
pub mod hotspots;
pub mod info;
// pub mod multisig;
pub mod router;
pub mod transfer;
pub mod upgrade;

/// Common options for most wallet commands
#[derive(Debug, clap::Args)]
pub struct Opts {
    /// File(s) to use
    #[arg(
        short = 'f',
        long = "file",
        number_of_values(1),
        default_value = "wallet.key"
    )]
    files: Vec<PathBuf>,

    /// Solana RPC URL to use.
    #[arg(long, default_value = "m")]
    url: String,
}

#[derive(Debug, Clone, clap::Args)]
pub struct CommitOpts {
    /// Commit the transaction
    #[arg(long)]
    commit: bool,

    /// Skip preflight checks when committing transaction. [default: false]
    #[arg(long)]
    skip_preflight: bool,
}

impl CommitOpts {
    pub fn maybe_commit(
        &self,
        tx: &solana_sdk::transaction::Transaction,
        client: &Client,
    ) -> Result {
        if self.commit {
            let signature = client.send_and_confirm_transaction(tx, self.skip_preflight)?;
            print_commit_result(signature)
        } else {
            let result = client.simulate_transaction(tx)?;
            print_simulation_response(&result)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Transaction(BlockchainTxn);

impl std::str::FromStr for Transaction {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(Self(b64::decode_message(s)?))
    }
}

fn load_wallet(files: &[PathBuf]) -> Result<Wallet> {
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

fn get_wallet_password(confirm: bool) -> std::io::Result<String> {
    match env::var("HELIUM_WALLET_PASSWORD") {
        Ok(str) => Ok(str),
        _ => get_password("Wallet Password", confirm),
    }
}

fn get_password(prompt: &str, confirm: bool) -> std::io::Result<String> {
    use dialoguer::Password;
    let mut builder = Password::new();
    builder.with_prompt(prompt);
    if confirm {
        builder.with_confirmation("Confirm password", "Passwords do not match");
    };
    builder.interact()
}

fn new_client(url: &str) -> Result<Arc<Client>> {
    let url = match url {
        "m" | "mainnet-beta" => "https://solana-rpc.web.helium.io:443",
        "d" | "devnet" => "https://solana-rpc.web.test-helium.com",
        url => url,
    };
    Ok(Arc::new(Client::new(url)?))
}

// fn read_txn(txn: &Option<Transaction>) -> Result<BlockchainTxn> {
//     match txn {
//         Some(txn) => Ok(txn.0.clone()),
//         None => {
//             let mut buffer = String::new();
//             io::stdin().read_line(&mut buffer)?;
//             Ok(buffer.trim().parse::<Transaction>()?.0)
//         }
//     }
// }

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

pub fn print_json<T: ?Sized + serde::Serialize>(value: &T) -> Result {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn print_commit_result(signature: solana_sdk::signature::Signature) -> Result {
    let json = json!({
        "result": "ok",
        "txid": signature.to_string(),
    });
    print_json(&json)
}

pub fn print_simulation_response(
    result: &solana_client::rpc_response::RpcSimulateTransactionResult,
) -> Result {
    if result.err.is_some() {
        let _ = print_json(&result);
        bail!("Transaction simulation failed");
    }
    print_json(&json!({
        "result": "ok",
    }))
}
