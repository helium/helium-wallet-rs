use crate::{
    result::{anyhow, bail, Error, Result},
    wallet::Wallet,
};
use dialoguer::Confirm;
use helium_lib::{
    b64,
    blockchain_api::{
        self,
        types::{ApiTransactions, StatusResponse, TokenAmount, TransactionData},
    },
    client::{self, SolanaRpcClient},
    keypair::{to_pubkey, Keypair, Pubkey, Signature, Signer},
    solana_client::{
        self, rpc_request::RpcResponseErrorData, rpc_response::RpcSimulateTransactionResult,
    },
    solana_sdk::transaction::VersionedTransaction,
};
use serde_json::json;
use std::{
    env, fs, io,
    io::IsTerminal,
    ops::Deref,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, OnceLock},
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
#[derive(clap::Args)]
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

    /// Memoized client, built once on first `client()` call. Every command's
    /// `run()` calls `opts.client()`; without this each invocation would spin
    /// up a second reqwest pool and RPC client distinct from the one used to
    /// seed the global KTA cache in `main`. `clap(skip)` keeps it out of the
    /// argument parser.
    #[arg(skip)]
    client: OnceLock<client::Client>,
}

// Manual impl: `client::Client` holds reqwest/RPC handles that don't
// implement `Debug`, and the cache state isn't useful debug output anyway.
impl std::fmt::Debug for Opts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opts")
            .field("files", &self.files)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
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

    /// Return the shared client for this invocation, building it once and
    /// caching it. Subsequent calls (and the per-command `run()` calls) hand
    /// back a clone of the same `Arc`-backed handles, so the whole process
    /// shares a single reqwest pool and RPC client — the same instance used to
    /// seed the global KTA cache.
    pub fn client(&self) -> Result<client::Client> {
        if let Some(client) = self.client.get() {
            return Ok(client.clone());
        }
        let client = client::Client::try_from(self.url.as_str())?;
        // Ignore the Err: a concurrent caller may have set it first, in which
        // case `get()` below returns that instance and ours is dropped.
        let _ = self.client.set(client);
        Ok(self
            .client
            .get()
            .expect("client set above or by a concurrent caller")
            .clone())
    }

    /// Build a blockchain-api client for this invocation's cluster.
    ///
    /// Uses `HELIUM_BLOCKCHAIN_API_URL` if set; otherwise defaults to the
    /// mainnet endpoint. Devnet has no built-in default yet, so it requires the
    /// environment variable to be set explicitly.
    pub fn blockchain_api(&self) -> Result<blockchain_api::Client> {
        if let Ok(url) = env::var(blockchain_api::API_URL_ENV) {
            return Ok(blockchain_api::Client::new(url));
        }
        if client::is_devnet(&self.url) {
            bail!(
                "no default blockchain-api URL for devnet; set {}",
                blockchain_api::API_URL_ENV
            );
        }
        // Default to the mainnet API. Warn when the RPC cluster isn't a
        // recognized mainnet alias, so a custom/other-cluster `--url` doesn't
        // silently build and submit against mainnet.
        if !matches!(self.url.as_str(), "m" | "mainnet" | "mainnet-beta") {
            eprintln!(
                "warning: {env} is unset; defaulting to the mainnet blockchain-api \
                 ({DEFAULT_BLOCKCHAIN_API_URL}) while --url is {url:?}. Set {env} if this \
                 cluster uses a different blockchain-api.",
                env = blockchain_api::API_URL_ENV,
                url = self.url,
            );
        }
        Ok(blockchain_api::Client::new(DEFAULT_BLOCKCHAIN_API_URL))
    }
}

/// Default mainnet blockchain-api base URL, used when
/// `HELIUM_BLOCKCHAIN_API_URL` is unset.
pub const DEFAULT_BLOCKCHAIN_API_URL: &str = "https://my-helium.web.helium.io/api/v1";

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

/// Map a Solana client error into our `Error`, lifting any preflight
/// simulation logs and RPC message into the error context so a failed
/// send/simulate reports *why* (program logs) rather than an opaque code.
fn context_err(client_err: solana_client::client_error::ClientError) -> Error {
    let mut captured_logs: Option<Vec<String>> = None;
    let mut error_message: Option<String> = None;
    if let solana_client::client_error::ClientErrorKind::RpcError(
        solana_client::rpc_request::RpcError::RpcResponseError {
            data:
                RpcResponseErrorData::SendTransactionPreflightFailure(RpcSimulateTransactionResult {
                    logs,
                    ..
                }),
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

impl CommitOpts {
    /// Sign and commit transactions built by the blockchain-api.
    ///
    /// Decodes the unsigned transactions in `response`, shows them for review,
    /// signs each locally with `signer`, then:
    /// - with `--commit`: sign and submit the batch via the API, polling until
    ///   it reaches a terminal status;
    /// - without `--commit`, interactive: simulate, then prompt to commit; sign
    ///   and submit only on confirmation;
    /// - without `--commit`, non-interactive (piped/CI): simulate and stop,
    ///   broadcasting nothing — so scripts never hang on a prompt or submit
    ///   unexpectedly.
    ///
    /// Signing happens only once a submit is decided, so a Ledger is never
    /// touched for a dry run or a declined action. `signing` controls how:
    /// [`ApiSigning::FreshBlockhash`] refreshes the blockhash and signs as sole
    /// signer; [`ApiSigning::PreserveCosigned`] fills only the wallet's slot,
    /// keeping the server's message/blockhash/co-signatures intact.
    ///
    /// The blockchain-api sets priority fee, compute budget, and lookup tables
    /// server-side; there are no local priority-fee knobs.
    pub async fn commit_via_api<C, R>(
        &self,
        api: &blockchain_api::Client,
        rpc: &C,
        response: &R,
        signer: &(dyn Signer + Send + Sync),
        signing: ApiSigning,
    ) -> Result<CommitResponse>
    where
        C: AsRef<SolanaRpcClient>,
        R: ApiTransactions,
    {
        let data = response.transaction_data();
        let unsigned = data.decode_transactions()?;
        display_action_for_review(data, response.estimated_sol_fee(), &unsigned);

        let solana = rpc.as_ref();

        // Decide whether to submit BEFORE signing, so a Ledger is never touched
        // for a dry run or a declined action.
        let submit = if self.commit {
            true
        } else {
            // Simulate for feedback, then either ask (interactive) or stop
            // (non-interactive — a safe no-op so scripts never hang on a prompt
            // or submit unexpectedly).
            simulate_unsigned(solana, &unsigned).await?;
            if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
                Confirm::new()
                    .with_prompt("Commit and submit these transaction(s)?")
                    .default(false)
                    .interact()?
            } else {
                false
            }
        };

        if !submit {
            return Ok(CommitResponse::None);
        }

        let signed = sign_for_submit(solana, unsigned, signer, signing).await?;
        let submitted = api.submit_signed(data, &signed, true).await?;
        let timeout = Duration::from_secs(self.confirm_timeout_secs);
        let status = api
            .poll_status(
                &submitted.batch_id,
                blockchain_api::DEFAULT_POLL_INTERVAL,
                timeout,
            )
            .await?;
        commit_response_from_status(status)
    }
}

/// Sign the server-built transactions for submission. Fetches a fresh blockhash
/// only here (not on the dry-run path), and only for the [`ApiSigning::FreshBlockhash`]
/// case; [`ApiSigning::PreserveCosigned`] keeps the server's message intact.
async fn sign_for_submit(
    solana: &SolanaRpcClient,
    unsigned: Vec<VersionedTransaction>,
    signer: &(dyn Signer + Send + Sync),
    signing: ApiSigning,
) -> Result<Vec<VersionedTransaction>> {
    match signing {
        ApiSigning::FreshBlockhash => {
            let (blockhash, _) = solana
                .get_latest_blockhash_with_commitment(solana.commitment())
                .await?;
            unsigned
                .into_iter()
                .map(|mut tx| {
                    tx.message.set_recent_blockhash(blockhash);
                    VersionedTransaction::try_new(tx.message, &[signer]).map_err(Error::from)
                })
                .collect()
        }
        ApiSigning::PreserveCosigned => unsigned
            .into_iter()
            .map(|tx| sign_owner_in_place(tx, signer))
            .collect(),
    }
}

/// Simulate each unsigned transaction (no signature check, server-replaced
/// blockhash) to surface errors before signing/submitting. Includes the program
/// logs in the error so a failing CPI is diagnosable.
async fn simulate_unsigned(
    solana: &SolanaRpcClient,
    unsigned: &[VersionedTransaction],
) -> Result<()> {
    use helium_lib::solana_client::rpc_config::RpcSimulateTransactionConfig;
    let config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        ..Default::default()
    };
    for tx in unsigned {
        let result = solana
            .simulate_transaction_with_config(tx, config.clone())
            .await
            .map_err(context_err)?;
        if let Some(err) = result.value.err {
            let logs = result.value.logs.unwrap_or_default().join("\n");
            return Err(if logs.is_empty() {
                anyhow!("transaction simulation failed: {err}")
            } else {
                anyhow!("transaction simulation failed: {err}\n{logs}")
            });
        }
    }
    Ok(())
}

/// How [`CommitOpts::commit_via_api`] signs the transactions the blockchain-api
/// built.
#[derive(Debug, Clone, Copy)]
pub enum ApiSigning {
    /// Refresh the blockhash and sign as the sole signer. The default for the
    /// server-built, single-signer transactions the API returns.
    FreshBlockhash,
    /// Preserve the message, blockhash, and any co-signatures already present,
    /// filling in only the wallet's slot. Required for the ECC-verifier-co-signed
    /// data-only issue transaction.
    PreserveCosigned,
}

/// Fill in the wallet's signature on a server-built transaction without
/// altering the message, its blockhash, or any co-signatures already present.
/// Locates the wallet among the required signers and writes only that slot;
/// other signers' slots are left untouched.
fn sign_owner_in_place(
    mut tx: VersionedTransaction,
    signer: &(dyn Signer + Send + Sync),
) -> Result<VersionedTransaction> {
    let owner = signer.pubkey();
    let index = tx
        .message
        .static_account_keys()
        .iter()
        .position(|key| *key == owner)
        .filter(|index| *index < tx.message.header().num_required_signatures as usize)
        .ok_or_else(|| anyhow!("wallet is not a required signer of the transaction"))?;

    // A well-formed tx sizes `signatures` to `num_required_signatures`; guard
    // against a malformed server response so a short vector errors cleanly
    // instead of panicking on the index.
    if index >= tx.signatures.len() {
        bail!(
            "transaction has {} signature slot(s) but the wallet must sign at index {index}",
            tx.signatures.len(),
        );
    }

    let message_data = tx.message.serialize();
    let signature = signer
        .try_sign_message(&message_data)
        .map_err(|err| anyhow!("failed to sign transaction: {err}"))?;
    tx.signatures[index] = signature;
    Ok(tx)
}

/// Map a terminal blockchain-api batch status into a [`CommitResponse`].
/// Only a fully `Confirmed` batch is success; anything else surfaces the
/// per-transaction statuses so the failure is legible.
fn commit_response_from_status(status: StatusResponse) -> Result<CommitResponse> {
    use blockchain_api::types::BatchStatus;
    if status.status != BatchStatus::Confirmed {
        let details = status
            .transactions
            .iter()
            .map(|tx| format!("{} → {:?}", tx.signature, tx.status))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "blockchain-api batch {} ended in status {:?} [{details}]",
            status.batch_id,
            status.status,
        );
    }
    let signatures = status
        .transactions
        .iter()
        .map(|tx| Signature::from_str(&tx.signature))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| anyhow!("blockchain-api returned an unparseable signature: {err}"))?;
    match signatures.as_slice() {
        [] => bail!(
            "blockchain-api confirmed batch {} but returned no signatures",
            status.batch_id
        ),
        [single] => Ok(CommitResponse::Signature(*single)),
        _ => Ok(CommitResponse::Batch { signatures }),
    }
}

/// Build the human-readable review lines for API-built transactions. Combines
/// the server's declared intent (per-transaction `description` and the
/// estimated fee) with facts derived from the decoded transaction itself (fee
/// payer and instruction count) so the signer can confirm they are authorizing
/// what they intended. Returned separately from printing so it can be tested.
fn review_lines(
    data: &TransactionData,
    fee: Option<&TokenAmount>,
    unsigned: &[VersionedTransaction],
) -> Vec<String> {
    let fee_summary = match fee {
        Some(fee) => format!("est. fee ~{} SOL", fee.ui_amount_string),
        None => "fee set server-side".to_string(),
    };
    let mut lines = vec![format!(
        "{} transaction(s) built by the blockchain-api, {fee_summary}",
        unsigned.len(),
    )];
    for (i, (tx, item)) in unsigned.iter().zip(&data.transactions).enumerate() {
        let description = item
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("description"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("(no description)");
        let payer = tx
            .message
            .static_account_keys()
            .first()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<none>".to_string());
        lines.push(format!(
            "[{i}] {description} (fee payer {payer}, {} instruction(s))",
            tx.message.instructions().len(),
        ));
    }
    lines
}

/// Print the review summary to stderr before transactions are signed. For
/// Ledger sources the device additionally shows a blind-sign hash (see
/// [`print_blind_sign_hash`]).
fn display_action_for_review(
    data: &TransactionData,
    fee: Option<&TokenAmount>,
    unsigned: &[VersionedTransaction],
) {
    for line in review_lines(data, fee, unsigned) {
        eprintln!("→ {line}");
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
    /// A multi-transaction blockchain-api batch that fully confirmed.
    Batch {
        signatures: Vec<helium_lib::keypair::Signature>,
    },
    None,
}

impl CommitResponse {
    /// Whether transactions were actually submitted on-chain (as opposed to a
    /// dry run or declined commit). True for both a single signature and a
    /// confirmed batch.
    pub fn committed(&self) -> bool {
        matches!(self, Self::Signature(_) | Self::Batch { .. })
    }
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
            Self::Batch { signatures } => json!({
                "result": "ok",
                "committed": true,
                "txids": signatures.iter().map(ToString::to_string).collect::<Vec<_>>(),
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

    #[test]
    fn commit_response_batch_serializes_txids() {
        let signatures = vec![Signature::from([1u8; 64]), Signature::from([2u8; 64])];
        let value = CommitResponse::Batch {
            signatures: signatures.clone(),
        }
        .to_json();
        assert_eq!(value["result"], json!("ok"));
        assert_eq!(value["committed"], json!(true));
        assert_eq!(
            value["txids"],
            json!(signatures
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>())
        );
    }

    fn status_with(
        status: helium_lib::blockchain_api::types::BatchStatus,
        signatures: &[Signature],
    ) -> StatusResponse {
        use helium_lib::blockchain_api::types::TransactionStatus;
        StatusResponse {
            batch_id: "batch".to_string(),
            status,
            submission_type: "single".to_string(),
            parallel: false,
            transactions: signatures
                .iter()
                .map(|sig| TransactionStatus {
                    signature: sig.to_string(),
                    status,
                })
                .collect(),
            jito_bundle_id: None,
        }
    }

    #[test]
    fn commit_from_status_confirmed_single_is_signature() {
        use helium_lib::blockchain_api::types::BatchStatus;
        let sig = Signature::from([3u8; 64]);
        let response = commit_response_from_status(status_with(BatchStatus::Confirmed, &[sig]))
            .expect("confirmed single maps to a signature");
        match response {
            CommitResponse::Signature(got) => assert_eq!(got, sig),
            other => panic!("expected Signature, got {other:?}"),
        }
    }

    #[test]
    fn commit_from_status_confirmed_multi_is_batch() {
        use helium_lib::blockchain_api::types::BatchStatus;
        let sigs = [Signature::from([4u8; 64]), Signature::from([5u8; 64])];
        let response = commit_response_from_status(status_with(BatchStatus::Confirmed, &sigs))
            .expect("confirmed multi maps to a batch");
        match response {
            CommitResponse::Batch { signatures } => assert_eq!(signatures, sigs.to_vec()),
            other => panic!("expected Batch, got {other:?}"),
        }
    }

    #[test]
    fn commit_from_status_non_confirmed_errors() {
        use helium_lib::blockchain_api::types::BatchStatus;
        let sig = Signature::from([6u8; 64]);
        assert!(commit_response_from_status(status_with(BatchStatus::Failed, &[sig])).is_err());
    }

    #[test]
    fn review_lines_include_intent_fee_and_payer() {
        use helium_lib::{
            blockchain_api::types::{
                ActionResponse, TokenAmount, TransactionData, TransactionItem,
            },
            solana_sdk::{
                hash::Hash,
                message::{Message, MessageHeader, VersionedMessage},
            },
        };

        let payer = Pubkey::new_unique();
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message {
                header: MessageHeader {
                    num_required_signatures: 0,
                    num_readonly_signed_accounts: 0,
                    num_readonly_unsigned_accounts: 0,
                },
                account_keys: vec![payer],
                recent_blockhash: Hash::default(),
                instructions: vec![],
            }),
        };
        let response = ActionResponse {
            transaction_data: TransactionData {
                transactions: vec![TransactionItem {
                    serialized_transaction: "unused".to_string(),
                    metadata: Some(
                        json!({ "type": "token_transfer", "description": "Transfer HNT" }),
                    ),
                }],
                parallel: false,
                tag: None,
                action_metadata: None,
            },
            estimated_sol_fee: TokenAmount {
                amount: "895934".to_string(),
                decimals: 9,
                ui_amount: Some(0.000895934),
                ui_amount_string: "0.000895934".to_string(),
                mint: "So11111111111111111111111111111111111111112".to_string(),
            },
        };

        let lines = review_lines(
            &response.transaction_data,
            Some(&response.estimated_sol_fee),
            &[tx],
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("est. fee ~0.000895934 SOL"));
        assert!(lines[1].contains("Transfer HNT"));
        assert!(lines[1].contains(&payer.to_string()));
    }
}
