use crate::{
    result::{anyhow, bail, Error, Result},
    wallet::Wallet,
};
use helium_lib::{
    b64,
    client::{self, SolanaRpcClient},
    keypair::Keypair,
    message, priority_fee,
    solana_client::{
        self, rpc_config::RpcSendTransactionConfig, rpc_request::RpcResponseErrorData,
        rpc_response::RpcSimulateTransactionResult,
    },
    solana_sdk::transaction::VersionedTransaction,
    TransactionOpts,
};
use serde_json::json;
use std::{
    env, fs, io,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

pub mod assets;
pub mod balance;
pub mod burn;
pub mod create;
pub mod dc;
pub mod export;
pub mod hotspots;
pub mod info;
pub mod memo;
pub mod price;
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

    pub fn client(&self) -> Result<client::Client> {
        Ok(client::Client::try_from(self.url.as_str())?)
    }
}

#[derive(Debug, Clone, clap::Args)]
pub struct CommitOpts {
    /// Skip pre-flight
    #[arg(long)]
    skip_preflight: bool,
    /// Minimum priority fee in micro lamports
    #[arg(long, default_value_t = priority_fee::MIN_PRIORITY_FEE)]
    min_priority_fee: u64,
    /// Maximum priority fee in micro lamports
    #[arg(long, default_value_t = priority_fee::MAX_PRIORITY_FEE)]
    max_priority_fee: u64,
    /// Commit the transaction
    #[arg(long)]
    commit: bool,
}

impl CommitOpts {
    pub async fn maybe_commit<C: AsRef<client::SolanaRpcClient>, T: Into<VersionedTransaction>>(
        &self,
        tx: T,
        client: &C,
    ) -> Result<CommitResponse> {
        fn context_err(client_err: solana_client::client_error::ClientError) -> Error {
            let mut captured_logs: Option<Vec<String>> = None;
            let mut error_message: Option<String> = None;
            if let solana_client::client_error::ClientErrorKind::RpcError(
                solana_client::rpc_request::RpcError::RpcResponseError {
                    data:
                        RpcResponseErrorData::SendTransactionPreflightFailure(
                            RpcSimulateTransactionResult { logs, .. },
                        ),
                    message,
                    ..
                },
            ) = &client_err.kind
            {
                logs.clone_into(&mut captured_logs);
                error_message = Some(message.clone());
            }
            let mut mapped = Error::from(client_err);
            if let Some(message) = error_message {
                mapped = mapped.context(message);
            }
            if let Some(logs) = captured_logs.as_ref() {
                if let Ok(serialized_logs) = serde_json::to_string(logs) {
                    mapped = mapped.context(serialized_logs);
                }
            }
            mapped
        }

        let versioned_tx = tx.into();
        if self.commit {
            let config = RpcSendTransactionConfig {
                skip_preflight: self.skip_preflight,
                ..Default::default()
            };
            client
                .as_ref()
                .send_transaction_with_config(&versioned_tx, config)
                .await
                .map(Into::into)
                .map_err(context_err)
        } else {
            client
                .as_ref()
                .simulate_transaction(&versioned_tx)
                .await
                .map_err(context_err)?
                .value
                .try_into()
        }
    }

    pub fn transaction_opts<C: AsRef<SolanaRpcClient>>(&self, client: &C) -> TransactionOpts {
        TransactionOpts {
            min_priority_fee: self.min_priority_fee,
            max_priority_fee: self.max_priority_fee,
            lut_addresses: if client::is_devnet(&client.as_ref().url()) {
                vec![message::COMMON_LUT_DEVNET]
            } else {
                vec![message::COMMON_LUT]
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Transaction(helium_proto::BlockchainTxn);

impl Deref for Transaction {
    type Target = helium_proto::BlockchainTxn;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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

pub fn phrase_to_words(phrase: &str) -> Vec<&str> {
    phrase.split_whitespace().collect()
}

pub trait ToJson {
    fn to_json(&self) -> serde_json::Value;
}
