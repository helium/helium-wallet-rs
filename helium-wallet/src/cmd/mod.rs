use crate::{
    result::{anyhow, bail, Error, Result},
    wallet::Wallet,
};
use helium_lib::{
    b64,
    keypair::Keypair,
    settings::Settings,
    solana_client::{
        self, rpc_request::RpcResponseErrorData, rpc_response::RpcSimulateTransactionResult,
    },
};
use serde_json::json;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

// pub mod assets;
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

    pub fn load_keypair(&self, password: &[u8]) -> Result<Arc<Keypair>> {
        let wallet = self.load_wallet()?;
        wallet.decrypt(password)
    }
}

impl TryFrom<Opts> for Settings {
    type Error = Error;
    fn try_from(value: Opts) -> Result<Self> {
        Ok(Settings::try_from(value.url.as_str())?)
    }
}

#[derive(Debug, Clone, clap::Args)]
pub struct CommitOpts {
    /// Commit the transaction
    #[arg(long)]
    commit: bool,
}

impl CommitOpts {
    pub async fn maybe_commit(
        &self,
        tx: &helium_lib::solana_sdk::transaction::Transaction,
        settings: &Settings,
    ) -> Result<CommitResponse> {
        fn context_err(client_err: solana_client::client_error::ClientError) -> Error {
            let mut captured_logs: Option<Vec<String>> = None;
            if let solana_client::client_error::ClientErrorKind::RpcError(
                solana_client::rpc_request::RpcError::RpcResponseError {
                    data:
                        RpcResponseErrorData::SendTransactionPreflightFailure(
                            RpcSimulateTransactionResult { logs, .. },
                        ),
                    ..
                },
            ) = &client_err.kind
            {
                logs.clone_into(&mut captured_logs);
            }
            let mut mapped = Error::from(client_err);
            if let Some(logs) = captured_logs.as_ref() {
                if let Ok(serialized_logs) = serde_json::to_string(logs) {
                    mapped = mapped.context(serialized_logs);
                }
            }
            mapped
        }

        let client = settings.mk_solana_client()?;
        if self.commit {
            client
                .send_transaction(tx)
                .await
                .map(Into::into)
                .map_err(context_err)
        } else {
            client
                .simulate_transaction(tx)
                .await
                .map_err(context_err)?
                .value
                .try_into()
        }
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

#[derive(Debug, serde::Serialize)]
pub enum CommitResponse {
    Signature(helium_lib::keypair::Signature),
    None,
}

impl From<helium_lib::keypair::Signature> for CommitResponse {
    fn from(value: helium_lib::keypair::Signature) -> Self {
        Self::Signature(value)
    }
}

impl TryFrom<solana_client::rpc_response::RpcSimulateTransactionResult> for CommitResponse {
    type Error = Error;
    fn try_from(
        value: solana_client::rpc_response::RpcSimulateTransactionResult,
    ) -> Result<CommitResponse> {
        if let Some(err) = value.err {
            Err(err.into())
        } else {
            Ok(Self::None)
        }
    }
}

impl ToJson for CommitResponse {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Signature(signature) => json!({
                "result": "ok",
                "txid": signature.to_string(),
            }),
            Self::None => json!({"result": "ok"}),
        }
    }
}

impl ToJson for Result<CommitResponse> {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Ok(response) => response.to_json(),
            Err(err) => json!({
                "result": "error",
                "error": err.to_string()
            }),
        }
    }
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

pub trait ToJson {
    fn to_json(&self) -> serde_json::Value;
}
