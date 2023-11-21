use crate::{
    b64,
    keypair::Keypair,
    result::{bail, Error, Result},
    settings::Settings,
    wallet::Wallet,
};
use anchor_client::{solana_client, solana_sdk};
use serde_json::json;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    rc::Rc,
};

pub mod balance;
pub mod create;
pub mod dc;
pub mod export;
pub mod hotspots;
pub mod info;
pub mod router;
pub mod sign;
pub mod transfer;
pub mod upgrade;

/// Common options for most wallet commands
#[derive(Debug, clap::Args, Clone)]
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

impl Opts {
    pub fn load_wallet(&self) -> Result<Wallet> {
        let mut files_iter = self.files.iter();
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

    pub fn load_keypair(&self, password: &[u8]) -> Result<Rc<Keypair>> {
        let wallet = self.load_wallet()?;
        wallet.decrypt(password)
    }
}

impl TryFrom<Opts> for Settings {
    type Error = Error;
    fn try_from(value: Opts) -> Result<Self> {
        Settings::try_from(value.url.as_str())
    }
}

#[derive(Debug, Clone, clap::Args)]
pub struct CommitOpts {
    /// Commit the transaction
    #[arg(long)]
    commit: bool,
}

impl CommitOpts {
    pub fn maybe_commit_quiet(
        &self,
        tx: &solana_sdk::transaction::Transaction,
        settings: &Settings,
        quiet: bool,
    ) -> Result {
        let client = settings.mk_solana_client()?;
        if self.commit {
            let signature = client.send_and_confirm_transaction(tx)?;
            if !quiet {
                print_commit_result(signature)?;
            }
        } else {
            let result = client.simulate_transaction(tx)?.value;
            if !quiet {
                print_simulation_response(&result)?;
            }
        }
        Ok(())
    }

    pub fn maybe_commit(
        &self,
        tx: &solana_sdk::transaction::Transaction,
        settings: &Settings,
    ) -> Result {
        self.maybe_commit_quiet(tx, settings, false)
    }
}

#[derive(Debug, Clone)]
pub struct Transaction(helium_proto::BlockchainTxn);

impl std::str::FromStr for Transaction {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(Self(b64::decode_message(s)?))
    }
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

fn read_txn(txn: &Option<Transaction>) -> Result<helium_proto::BlockchainTxn> {
    match txn {
        Some(txn) => Ok(txn.0.clone()),
        None => {
            let mut buffer = String::new();
            io::stdin().read_line(&mut buffer)?;
            Ok(buffer.trim().parse::<Transaction>()?.0)
        }
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
pub fn phrase_to_words(phrase: &str) -> Vec<String> {
    phrase.split_whitespace().map(|w| w.to_string()).collect()
}
