use crate::{
    result::{anyhow, bail, Error, Result},
    wallet::Wallet,
};
use helium_lib::{
    b64,
    client::{self, SolanaRpcClient},
    keypair::{to_pubkey, Keypair, Pubkey, Signer},
    priority_fee,
    solana_client::{
        self, rpc_config::RpcSendTransactionConfig, rpc_request::RpcResponseErrorData,
        rpc_response::RpcSimulateTransactionResult,
    },
    solana_sdk::{commitment_config::CommitmentConfig, transaction::VersionedTransaction},
    transaction::{self as lib_transaction, SignatureStatus},
    TransactionOpts,
};
use serde_json::json;
use std::{
    env, fs, io,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

pub mod assets;
pub mod balance;
pub mod burn;
pub mod completion;
pub mod create;
pub mod dc;
pub mod export;
pub mod hotspots;
pub mod info;
pub mod ledger;
pub mod memo;
pub mod price;
pub mod router;
pub mod sign;
pub mod source;
pub mod squads;
pub mod swap;
pub mod transfer;
pub mod upgrade;

pub use source::WalletSource;

/// Common options for most wallet commands
#[derive(Debug, clap::Args, Clone)]
pub struct Opts {
    /// Wallet source(s) to use. Either a path to an encrypted key file, or a
    /// Ledger device URL (`usb://ledger?key=<account>/<change>`).
    #[arg(
        short = 'f',
        long = "file",
        number_of_values(1),
        default_value = "wallet.key"
    )]
    files: Vec<WalletSource>,

    /// Solana RPC URL to use.
    #[arg(long, default_value = "m")]
    url: String,
}

impl Opts {
    pub fn sources(&self) -> &[WalletSource] {
        &self.files
    }

    /// Resolve the wallet's Solana public key, opening a Ledger if the source
    /// requires it. Does not prompt for a password.
    pub fn load_pubkey(&self) -> Result<Pubkey> {
        match self.files.first() {
            None => bail!("at least one wallet source expected"),
            Some(WalletSource::Ledger { path, serial, .. }) => {
                if self.files.len() > 1 {
                    bail!("a Ledger source cannot be combined with other wallets");
                }
                let kp = helium_crypto::ledger::Keypair::from_derivation_path(
                    helium_crypto::Network::MainNet,
                    path.clone(),
                    serial.as_deref(),
                )?;
                to_pubkey(&kp.public_key).map_err(Error::from)
            }
            Some(WalletSource::File(_)) => Ok(self.load_wallet()?.public_key),
        }
    }

    pub fn maybe_wallet_key(&self, wallet: Option<Pubkey>) -> Result<Pubkey> {
        match wallet {
            Some(pubkey) => Ok(pubkey),
            None => self.load_pubkey(),
        }
    }

    pub fn load_wallet(&self) -> Result<Wallet> {
        let mut files_iter = self.files.iter().map(|s| match s {
            WalletSource::File(path) => Ok(path),
            WalletSource::Ledger { .. } => Err(anyhow!(
                "this command does not yet support Ledger sources; use a key file"
            )),
        });
        let mut first_wallet = match files_iter.next() {
            Some(path) => {
                let mut reader = fs::File::open(path?)?;
                Wallet::read(&mut reader)?
            }
            None => bail!("At least one wallet file expected"),
        };

        for path in files_iter {
            let mut reader = fs::File::open(path?)?;
            let w = Wallet::read(&mut reader)?;
            first_wallet.absorb_shard(&w)?;
        }

        Ok(first_wallet)
    }

    pub fn load_keypair(&self, password: &[u8]) -> Result<Arc<Keypair>> {
        let wallet = self.load_wallet()?;
        wallet.decrypt(password)
    }

    /// Resolve the wallet to a Solana SDK signer. For File sources this
    /// prompts for a password and decrypts the keyfile; for Ledger sources
    /// it opens the device (no password). The returned signer can be passed
    /// to any helium-lib function expecting `&dyn Signer`.
    pub fn load_signer(&self) -> Result<Arc<dyn Signer + Send + Sync>> {
        match self.files.first() {
            None => bail!("at least one wallet source expected"),
            Some(WalletSource::Ledger { path, serial, .. }) => {
                if self.files.len() > 1 {
                    bail!("a Ledger source cannot be combined with other wallets");
                }
                let kp = helium_crypto::ledger::Keypair::from_derivation_path(
                    helium_crypto::Network::MainNet,
                    path.clone(),
                    serial.as_deref(),
                )?
                .with_blind_sign_hook(print_blind_sign_hash);
                Ok(Arc::new(kp))
            }
            Some(WalletSource::File(_)) => {
                let password = get_wallet_password(false)?;
                Ok(self.load_keypair(password.as_bytes())?)
            }
        }
    }

    pub fn client(&self) -> Result<client::Client> {
        Ok(client::Client::try_from(self.url.as_str())?)
    }
}

/// Blind-sign hook installed on every Ledger keypair we hand out. Fires
/// just before the device receives a SIGN_MESSAGE APDU that's going to
/// blind-sign (any program outside the Solana app's clear-sign whitelist).
/// Prints the SHA-256 the device will display in base58 to stderr so the
/// user can compare. helium-crypto no longer prints this itself; it's our
/// job to surface for any caller that wants the UX.
fn print_blind_sign_hash(hash: &[u8; 32]) {
    eprintln!(
        "→ Ledger blind-sign — verify hash on device: {}",
        bs58::encode(hash).into_string()
    );
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
    /// Seconds to wait for the submitted transaction to confirm. The wallet
    /// returns success only when the signature reaches `confirmed`
    /// commitment. Bump this if you're signing on a slow device (e.g.
    /// reading a blind-sign hash on a Ledger Flex) and the recent_blockhash
    /// might expire before submission.
    #[arg(long, default_value_t = 90)]
    confirm_timeout_secs: u64,
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
            let signature = client
                .as_ref()
                .send_transaction_with_config(&versioned_tx, config)
                .await
                .map_err(context_err)?;

            // Submission ≠ confirmation. Solana's send_transaction returns
            // the locally-computed signature regardless of whether the tx
            // ever lands — by far the most common Ledger failure mode is
            // the recent_blockhash expiring while the user reads the device
            // and approves. Poll for confirmation so a non-landing tx
            // surfaces as an error instead of looking identical to a
            // successful one. See `transaction::confirm_signatures` in
            // helium-lib for the underlying primitive.
            let timeout = Duration::from_secs(self.confirm_timeout_secs);
            let poll_interval = Duration::from_secs(2);
            let statuses = lib_transaction::confirm_signatures(
                client,
                &[signature],
                CommitmentConfig::confirmed(),
                timeout,
                poll_interval,
            )
            .await?;

            match statuses.get(&signature) {
                Some(SignatureStatus::Confirmed) => Ok(CommitResponse::Signature(signature)),
                Some(SignatureStatus::Failed(err)) => {
                    Err(anyhow!("transaction failed on-chain: {err}"))
                }
                Some(SignatureStatus::NotFound) | None => Err(anyhow!(
                    "transaction did not confirm within {}s — likely the blockhash \
                     expired while signing on the device, or the RPC dropped it before \
                     reaching leaders. Submitted signature: {signature}",
                    timeout.as_secs(),
                )),
            }
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
            ..TransactionOpts::for_client(client)
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

fn get_wallet_password(confirm: bool) -> std::io::Result<Zeroizing<String>> {
    match env::var("HELIUM_WALLET_PASSWORD") {
        Ok(str) => Ok(Zeroizing::new(str)),
        _ => get_password("Wallet Password", confirm),
    }
}

fn get_password(prompt: &str, confirm: bool) -> std::io::Result<Zeroizing<String>> {
    use dialoguer::Password;
    let mut builder = Password::new();
    builder.with_prompt(prompt);
    if confirm {
        builder.with_confirmation("Confirm password", "Passwords do not match");
    };
    builder.interact().map(Zeroizing::new)
}

/// Open `filename` for writing, restricting access to the owner on Unix.
/// Used for outputs that may contain key material.
pub fn open_output_file(filename: &Path, create: bool) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options
        .write(true)
        .create(true)
        .create_new(create)
        .truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(filename)
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
                "committed": true,
                "txid": signature.to_string(),
            }),
            Self::None => json!({
                "result": "ok",
                "committed": false,
            }),
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
    print_json(&CommitResponse::None.to_json())
}

pub fn phrase_to_words(phrase: &str) -> Vec<&str> {
    phrase.split_whitespace().collect()
}

pub trait ToJson {
    fn to_json(&self) -> serde_json::Value;
}

#[cfg(test)]
mod tests {
    use super::*;
    use helium_lib::keypair::Signature;

    #[test]
    fn commit_response_signature_serializes_with_committed_true() {
        let signature = Signature::from([7u8; 64]);
        let value = CommitResponse::Signature(signature).to_json();
        assert_eq!(value["result"], json!("ok"));
        assert_eq!(value["committed"], json!(true));
        assert_eq!(value["txid"], json!(signature.to_string()));
    }

    #[test]
    fn commit_response_none_serializes_with_committed_false() {
        let value = CommitResponse::None.to_json();
        assert_eq!(value["result"], json!("ok"));
        assert_eq!(value["committed"], json!(false));
        assert!(value.get("txid").is_none());
    }

    #[test]
    fn commit_response_error_keeps_existing_shape() {
        let err: Result<CommitResponse> = Err(anyhow!("boom"));
        let value = err.to_json();
        assert_eq!(value["result"], json!("error"));
        assert!(value.get("committed").is_none());
    }
}
