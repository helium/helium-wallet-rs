//! Wire types for the Helium blockchain-api REST surface (`/api/v1`).
//!
//! These mirror the Zod schemas published in the HPL repo's
//! `@helium/blockchain-api` package (`packages/blockchain-api-client/src/schemas`).
//! Every action endpoint takes the wallet pubkey as an explicit field and
//! returns an [`ActionResponse`]: an unsigned base64 `VersionedTransaction`
//! (or several) plus an estimated SOL fee. The caller decodes, signs, and
//! submits them back via [`super::Client::submit`].
//!
//! Amounts are raw base-unit ("bones") decimal strings on the wire; the
//! request constructors accept `u64` and stringify.

use crate::{
    error::{DecodeError, EncodeError},
    keypair::Pubkey,
    transaction::VersionedTransaction,
};
use serde::{Deserialize, Serialize};

/// Base64-decode a wire transaction into a `VersionedTransaction`.
///
/// The server serializes with web3.js `VersionedTransaction.serialize()`,
/// whose bytes are the canonical Solana wire format that `bincode` reads.
pub(super) fn decode_transaction(serialized: &str) -> Result<VersionedTransaction, DecodeError> {
    let bytes = crate::b64::decode(serialized)?;
    Ok(bincode::deserialize(&bytes)?)
}

/// Serialize a signed `VersionedTransaction` to base64 for submission.
pub(super) fn encode_transaction(tx: &VersionedTransaction) -> Result<String, EncodeError> {
    Ok(crate::b64::encode(bincode::serialize(tx)?))
}

// ---- Shared value types ----

/// Request-side token amount: raw base-unit string + mint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenAmountInput {
    /// Raw token amount in the smallest unit (bones).
    pub amount: String,
    /// Mint address of the token.
    pub mint: String,
}

impl TokenAmountInput {
    /// Build from a mint and a raw base-unit amount.
    pub fn new(mint: &Pubkey, amount: u64) -> Self {
        Self {
            amount: amount.to_string(),
            mint: mint.to_string(),
        }
    }
}

/// Rich token amount returned by the API (e.g. `estimatedSolFee`, pending
/// rewards): raw amount plus decimals and pre-formatted UI fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenAmount {
    /// Raw token amount in the smallest unit (bones).
    pub amount: String,
    /// Number of decimals for the mint.
    pub decimals: u8,
    /// Numeric amount if `<= Number.MAX_SAFE_INTEGER`, otherwise null.
    pub ui_amount: Option<f64>,
    /// String representation of `ui_amount`.
    pub ui_amount_string: String,
    /// Mint address of the token.
    pub mint: String,
}

// ---- Action response envelope ----

/// A single (unsigned, on the way out; signed, on the way back) transaction
/// plus opaque server metadata that must be round-tripped to `submit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionItem {
    /// Base64-encoded wire transaction.
    pub serialized_transaction: String,
    /// Server-attached metadata. Opaque here — forwarded verbatim to `submit`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub metadata: Option<serde_json::Value>,
}

/// The `transactionData` envelope returned by every action endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionData {
    /// One or more transactions to sign and submit, in order.
    pub transactions: Vec<TransactionItem>,
    /// Whether the transactions may be submitted in parallel.
    pub parallel: bool,
    /// Optional idempotency/grouping tag.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tag: Option<String>,
    /// Optional server-defined action metadata, forwarded verbatim to `submit`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub action_metadata: Option<serde_json::Value>,
}

impl TransactionData {
    /// Decode all transactions into `VersionedTransaction`s, preserving order.
    pub fn decode_transactions(&self) -> Result<Vec<VersionedTransaction>, DecodeError> {
        self.transactions
            .iter()
            .map(|item| decode_transaction(&item.serialized_transaction))
            .collect()
    }

    /// Build a [`SubmitRequest`] from the signed transactions, preserving each
    /// transaction's metadata and this envelope's `parallel`/`tag`/
    /// `action_metadata`. `signed` must be the same length and order as the
    /// transactions returned by the action endpoint.
    pub fn to_submit_request(
        &self,
        signed: &[VersionedTransaction],
        simulate: bool,
    ) -> Result<SubmitRequest, super::BlockchainApiError> {
        if signed.len() != self.transactions.len() {
            return Err(super::BlockchainApiError::config(format!(
                "signed transaction count {} does not match the {} returned by the API",
                signed.len(),
                self.transactions.len()
            )));
        }
        let transactions = signed
            .iter()
            .zip(&self.transactions)
            .map(|(tx, item)| {
                Ok(TransactionItem {
                    serialized_transaction: encode_transaction(tx)?,
                    metadata: item.metadata.clone(),
                })
            })
            .collect::<Result<Vec<_>, EncodeError>>()?;
        Ok(SubmitRequest {
            transactions,
            parallel: self.parallel,
            tag: self.tag.clone(),
            action_metadata: self.action_metadata.clone(),
            simulation_commitment: None,
            simulate: Some(simulate),
        })
    }
}

/// Standard response for every transaction-building action endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResponse {
    /// The unsigned transactions to sign and submit.
    pub transaction_data: TransactionData,
    /// Estimated total SOL fee (rent + priority fees + automation costs).
    pub estimated_sol_fee: TokenAmount,
}

impl ActionResponse {
    /// Decode the unsigned transactions into `VersionedTransaction`s.
    pub fn decode_transactions(&self) -> Result<Vec<VersionedTransaction>, DecodeError> {
        self.transaction_data.decode_transactions()
    }
}

/// Uniform access to the transactions (and optional fee estimate) an action
/// endpoint returns. Endpoints differ in envelope: most wrap the transactions
/// under `transactionData` with a sibling `estimatedSolFee`
/// ([`ActionResponse`]), while data-credit endpoints return the
/// [`TransactionData`] bare with no fee estimate. This trait lets a caller sign
/// and submit either shape uniformly.
pub trait ApiTransactions {
    /// The transactions to sign and submit.
    fn transaction_data(&self) -> &TransactionData;
    /// The estimated SOL fee, when the endpoint provides one.
    fn estimated_sol_fee(&self) -> Option<&TokenAmount>;
}

impl ApiTransactions for ActionResponse {
    fn transaction_data(&self) -> &TransactionData {
        &self.transaction_data
    }
    fn estimated_sol_fee(&self) -> Option<&TokenAmount> {
        Some(&self.estimated_sol_fee)
    }
}

impl ApiTransactions for TransactionData {
    fn transaction_data(&self) -> &TransactionData {
        self
    }
    fn estimated_sol_fee(&self) -> Option<&TokenAmount> {
        None
    }
}

// ---- Submit / status ----

/// Simulation commitment level for `submit`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SimulationCommitment {
    Confirmed,
    Finalized,
}

/// Request body for `POST /transactions`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRequest {
    /// Signed transactions to broadcast (max 5 per batch).
    pub transactions: Vec<TransactionItem>,
    /// Whether the batch may be submitted in parallel.
    pub parallel: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulation_commitment: Option<SimulationCommitment>,
    /// Simulate before broadcasting (server default: true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulate: Option<bool>,
}

/// Response from `POST /transactions`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitResponse {
    /// Identifier for polling batch status via `GET /transactions/{id}`.
    pub batch_id: String,
    /// Optional human-readable message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Terminal or in-progress status of a submitted batch or transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BatchStatus {
    Pending,
    Confirmed,
    Failed,
    Expired,
    Partial,
}

impl BatchStatus {
    /// Whether polling can stop (anything other than `Pending`).
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending)
    }

    /// Whether the batch fully confirmed.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

/// Per-transaction status within a batch.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionStatus {
    pub signature: String,
    pub status: BatchStatus,
}

/// Response from `GET /transactions/{id}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub batch_id: String,
    pub status: BatchStatus,
    /// `single` | `parallel` | `sequential` | `jito_bundle`. Kept as a string:
    /// we report it but never branch on it.
    pub submission_type: String,
    pub parallel: bool,
    pub transactions: Vec<TransactionStatus>,
    #[serde(default)]
    pub jito_bundle_id: Option<String>,
}

// ---- Action request bodies ----

/// The reward network for a claim.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RewardNetwork {
    Hnt,
    Iot,
    Mobile,
}

/// Hotspot device type for `update-info`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Iot,
    Mobile,
}

/// A latitude/longitude pair.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LatLng {
    pub lat: f64,
    pub lng: f64,
}

/// `POST /tokens/transfer` — single-recipient SPL transfer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTransferRequest {
    pub wallet_address: String,
    pub destination: String,
    pub token_amount: TokenAmountInput,
    /// If set, build the transfer as a Squads v4 proposal from this multisig's
    /// vault instead of a direct transfer. `wallet_address` is the proposing
    /// member and outer fee payer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multisig: Option<String>,
    /// Memo recorded on the Squads proposal (only meaningful with `multisig`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// A recipient in a multi-transfer.
#[derive(Debug, Clone, Serialize)]
pub struct Recipient {
    pub destination: String,
    /// Raw token amount in the smallest unit (bones).
    pub amount: String,
}

/// `POST /tokens/multi-transfer` — many recipients of the same mint, packed
/// into as few transactions as possible.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiTransferRequest {
    pub wallet_address: String,
    pub mint: String,
    pub recipients: Vec<Recipient>,
}

/// `POST /data-credits/mint` — mint DC by burning HNT.
///
/// Provide exactly one of `dc_amount` / `hnt_amount`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DcMintRequest {
    pub owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dc_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hnt_amount: Option<String>,
    /// Recipient of the minted DC. Defaults to `owner` server-side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
}

/// `POST /data-credits/delegate` — delegate DC to a router.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DcDelegateRequest {
    pub owner: String,
    pub router_key: String,
    /// Raw DC amount (DC has 0 decimals).
    pub amount: String,
    /// SubDAO token mint (MOBILE or IOT) selecting the target subDAO.
    pub mint: String,
    /// In-tx memo (direct mode) or the Squads proposal memo (`multisig` mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// If set, delegate from this multisig's vault as a Squads v4 proposal.
    /// `owner` is the proposing member and outer fee payer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multisig: Option<String>,
}

/// `POST /hotspots/claim-rewards`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimRewardsRequest {
    pub wallet_address: String,
    /// Reward network; server default is `hnt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<RewardNetwork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tuktuk: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_pending_rewards: Option<TokenAmount>,
}

/// `POST /hotspots/update-rewards-destination`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRewardsDestinationRequest {
    pub wallet_address: String,
    pub hotspot_pubkey: String,
    pub destination: String,
    /// Lazy distributors to update; the endpoint requires at least one.
    pub lazy_distributors: Vec<String>,
}

/// Mobile hotspot deployment info. Only WiFi is modeled; CBRS is defunct.
/// Serializes to the API's tagged shape, e.g. `{"type":"WIFI", …}`. Absent
/// fields are omitted; the server merges them with the current on-chain values.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum DeploymentInfo {
    #[serde(rename = "WIFI", rename_all = "camelCase")]
    Wifi {
        #[serde(skip_serializing_if = "Option::is_none")]
        antenna: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        elevation: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        azimuth: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mechanical_down_tilt: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        electrical_down_tilt: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        serial: Option<String>,
    },
}

/// `POST /hotspots/update-info` — assert location and device details.
///
/// The API models this as a union on `device_type`: IoT uses
/// `gain`/`elevation`; Mobile uses `deployment_info`. Both may set `location`.
/// Absent fields are omitted and the server keeps the current on-chain value.
/// Who pays for an info update.
///
/// `Maker` relays the onboarding server, where the maker co-signs and covers
/// the DC, so the wallet is neither the fee payer nor the only signature.
/// `Owner` builds the update instruction server-side and the hotspot's owner
/// pays the DC, the resize rent and the transaction fee.
///
/// A caller that signs alone needs `Owner`: a maker-paid transaction carries a
/// second required signature it cannot supply. The field is required here
/// rather than defaulted, because the wire default is `Maker` and a caller
/// that omitted it would receive a transaction it cannot use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateInfoFeePayer {
    Maker,
    Owner,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfoRequest {
    pub device_type: DeviceType,
    pub entity_pub_key: String,
    pub wallet_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<LatLng>,
    /// IoT only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain: Option<f64>,
    /// IoT only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elevation: Option<f64>,
    /// IoT only. Antenna azimuth in degrees (0-360).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azimuth: Option<f64>,
    pub fee_payer: UpdateInfoFeePayer,
    /// Mobile only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment_info: Option<DeploymentInfo>,
}

/// A swap quote from `GET /swap/quote` (Jupiter-backed). The typed fields
/// drive display and cost checks; `extra` captures the remainder (routePlan,
/// platformFee, contextSlot, …) so the quote round-trips verbatim as the
/// `quoteResponse` body for `POST /swap/instructions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapQuote {
    pub input_mint: String,
    pub in_amount: String,
    pub output_mint: String,
    pub out_amount: String,
    pub other_amount_threshold: String,
    pub swap_mode: String,
    pub slippage_bps: u32,
    pub price_impact_pct: String,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

/// `POST /swap/instructions` — build the swap transaction for a quote.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapInstructionsRequest {
    /// The quote returned by `swap/quote`, passed back verbatim.
    pub quote_response: SwapQuote,
    /// The wallet that will sign and pay for the swap.
    pub user_public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_token_account: Option<String>,
}

/// `POST /tokens/burn` — burn SPL tokens from the wallet's account.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenBurnRequest {
    pub wallet_address: String,
    pub token_amount: TokenAmountInput,
    /// If set, burn from this multisig's vault as a Squads v4 proposal.
    /// `wallet_address` is the proposing member and outer fee payer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multisig: Option<String>,
    /// Memo recorded on the Squads proposal (only with `multisig`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// `POST /tokens/memo` — emit a bare spl-memo transaction.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoRequest {
    pub wallet_address: String,
    pub memo: String,
}

/// `POST /data-credits/burn` — burn DC directly from the owner's account.
/// Returns the bare [`TransactionData`], like the other data-credit endpoints.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DcBurnRequest {
    pub owner: String,
    /// Raw DC amount (DC has 0 decimals).
    pub amount: String,
    /// If set, burn from this multisig's vault as a Squads v4 proposal.
    /// `owner` is the proposing member and outer fee payer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multisig: Option<String>,
    /// Memo recorded on the Squads proposal (only with `multisig`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// `POST /hotspots/burn` — permanently burn a hotspot cNFT.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotspotBurnRequest {
    pub wallet_address: String,
    pub hotspot_pubkey: String,
    /// If set, burn a hotspot the multisig's vault owns, as a Squads v4
    /// proposal. `wallet_address` is the proposing member and outer fee payer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multisig: Option<String>,
    /// Memo recorded on the Squads proposal (only with `multisig`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// `POST /hotspots/transfer` — transfer a hotspot cNFT to a new owner.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferHotspotRequest {
    pub wallet_address: String,
    pub hotspot_pubkey: String,
    pub recipient: String,
    /// If set, transfer a hotspot the multisig's vault owns, as a Squads v4
    /// proposal. `wallet_address` is the proposing member and outer fee payer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multisig: Option<String>,
    /// Memo recorded on the Squads proposal (only with `multisig`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// Body for `POST /hotspots/{entityPubKey}/claim-rewards`. The entity key is a
/// path parameter, so it is not part of this body.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimHotspotRewardsRequest {
    pub wallet_address: String,
    /// Reward network; server default is `hnt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<RewardNetwork>,
}

// ---- Data-only hotspot onboarding ----

/// IoT or Mobile sub-DAO selector for data-only onboarding.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DataOnlyNetwork {
    Iot,
    Mobile,
}

/// Body for `POST /hotspots/data-only/issue`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDataOnlyHotspotRequest {
    pub wallet_address: String,
    /// Base64 add-gateway token (BlockchainTxnAddGatewayV1 envelope).
    pub add_gateway_txn: String,
}

/// Body for `POST /hotspots/data-only/onboard`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardDataOnlyHotspotRequest {
    pub wallet_address: String,
    pub network: DataOnlyNetwork,
    /// Base58 helium public key of the hotspot to onboard.
    pub hotspot_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lng: Option<f64>,
    /// IoT only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elevation: Option<i32>,
    /// IoT only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain: Option<f64>,
}

// ---- Claim automation (one tuktuk claim cron per wallet) ----
//
// `wallet_address` carries the path parameter and is never serialized into the
// body: the endpoints take the wallet from the URL. The cron is set up empty and
// claims are attached separately, so a single cron can mix whole-wallet and
// per-hotspot claims.

/// Body for `POST /hotspots/wallet/{walletAddress}/automation`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupAutomationRequest {
    #[serde(skip_serializing)]
    pub wallet_address: String,
    /// How often the cron fires: a preset cadence (`daily`, `weekly`,
    /// `monthly`), or a raw crontab string in clockwork format. Presets are
    /// resolved to a crontab server-side.
    pub schedule: String,
    /// Number of claim cycles to pre-fund.
    pub duration: u32,
}

/// Body for `POST /hotspots/wallet/{walletAddress}/automation/fund`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundAutomationRequest {
    #[serde(skip_serializing)]
    pub wallet_address: String,
    /// Additional claim cycles to fund across both pools.
    pub additional_duration: u32,
}

/// Body for `POST /hotspots/wallet/{walletAddress}/automation/close`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseAutomationRequest {
    #[serde(skip_serializing)]
    pub wallet_address: String,
}

/// Body for `POST /hotspots/wallet/{walletAddress}/automation/requeue`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequeueAutomationRequest {
    #[serde(skip_serializing)]
    pub wallet_address: String,
}

/// Body for `POST /hotspots/wallet/{walletAddress}/automation/add-wallet`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddWalletToAutomationRequest {
    #[serde(skip_serializing)]
    pub wallet_address: String,
}

/// Body for `POST /hotspots/wallet/{walletAddress}/automation/add-entity`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddEntityToAutomationRequest {
    #[serde(skip_serializing)]
    pub wallet_address: String,
    /// Base58 helium public key of the hotspot to claim.
    pub entity_key: String,
}

/// Body for `POST /hotspots/wallet/{walletAddress}/automation/remove-entity`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveEntityFromAutomationRequest {
    #[serde(skip_serializing)]
    pub wallet_address: String,
    /// Cron-transaction index of the claim entry to remove.
    pub index: u32,
}

/// Body for `POST /hotspots/automation/top-up` — operator floor top-up. For each
/// target wallet whose cron/claim pool is at or below `floor_lamports`, the
/// operator funds it with `fund_lamports`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopUpAutomationRequest {
    pub operator_address: String,
    pub floor_lamports: u64,
    pub fund_lamports: u64,
    pub targets: Vec<String>,
}

// ---- Squads v4 proposal lifecycle ----

/// Shared body for the Squads v4 proposal votes: approve / reject / cancel.
/// The `member` casting the vote is also the outer fee payer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadsProposalVoteRequest {
    pub member: String,
    pub multisig: String,
    /// Transaction index of the target proposal (on-chain u64, sent as a string).
    pub transaction_index: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// `POST /squads/proposals/execute` — execute an approved proposal. Handles both
/// vault and config transactions; the server detects which the index holds.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadsExecuteProposalRequest {
    pub member: String,
    pub multisig: String,
    pub transaction_index: String,
}

/// A member permission bit for an [`SquadsConfigAction::AddMember`].
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SquadsPermission {
    Initiate,
    Vote,
    Execute,
}

/// A single config change in a [`SquadsProposeConfigChangeRequest`].
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SquadsConfigAction {
    #[serde(rename_all = "camelCase")]
    AddMember {
        new_member: String,
        /// Permissions granted to the new member; server defaults to all three.
        #[serde(skip_serializing_if = "Option::is_none")]
        permissions: Option<Vec<SquadsPermission>>,
    },
    #[serde(rename_all = "camelCase")]
    RemoveMember { old_member: String },
    #[serde(rename_all = "camelCase")]
    ChangeThreshold { new_threshold: u16 },
}

/// `POST /squads/proposals/config` — propose a config change (add/remove member,
/// change threshold). The server assigns the proposal's transaction index and
/// returns it in the response's `actionMetadata`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SquadsProposeConfigChangeRequest {
    pub member: String,
    pub multisig: String,
    pub actions: Vec<SquadsConfigAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

#[cfg(test)]
mod update_info_tests {
    use super::*;

    /// The wire values are the server's enum, and `maker` is its default: a
    /// caller that signs alone and sends the wrong one gets a transaction
    /// carrying a signature it cannot supply.
    #[test]
    fn the_fee_payer_serializes_to_the_values_the_server_accepts() {
        let owner = serde_json::to_value(UpdateInfoFeePayer::Owner).expect("serialize owner");
        let maker = serde_json::to_value(UpdateInfoFeePayer::Maker).expect("serialize maker");
        assert_eq!(owner, serde_json::json!("owner"));
        assert_eq!(maker, serde_json::json!("maker"));
    }

    #[test]
    fn the_request_names_the_fee_payer_on_the_wire() {
        let request = UpdateInfoRequest {
            device_type: DeviceType::Iot,
            entity_pub_key: "gw".to_string(),
            wallet_address: "wallet".to_string(),
            location: None,
            gain: None,
            elevation: None,
            azimuth: None,
            fee_payer: UpdateInfoFeePayer::Owner,
            deployment_info: None,
        };
        let json = serde_json::to_value(&request).expect("serialize the request");
        // camelCase, and present: omitted, the server applies `maker`.
        assert_eq!(json["feePayer"], serde_json::json!("owner"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::message::{Message, MessageHeader, VersionedMessage};

    #[test]
    fn action_response_deserializes_from_wire_shape() {
        // Fixed fixture matching the OpenAPI 200 shape for /tokens/transfer.
        let json = serde_json::json!({
            "transactionData": {
                "transactions": [{
                    "serializedTransaction": "AQID",
                    "metadata": { "type": "transfer", "description": "Send HNT" }
                }],
                "parallel": false,
                "tag": "abc"
            },
            "estimatedSolFee": {
                "amount": "5000",
                "decimals": 9,
                "uiAmount": 0.000005,
                "uiAmountString": "0.000005",
                "mint": "So11111111111111111111111111111111111111112"
            }
        });
        let resp: ActionResponse =
            serde_json::from_value(json).expect("deserialize ActionResponse");
        assert_eq!(resp.transaction_data.transactions.len(), 1);
        assert!(!resp.transaction_data.parallel);
        assert_eq!(resp.transaction_data.tag.as_deref(), Some("abc"));
        assert_eq!(resp.estimated_sol_fee.decimals, 9);
        assert_eq!(resp.estimated_sol_fee.amount, "5000");
    }

    #[test]
    fn bare_transaction_data_deserializes_dc_envelope() {
        // The data-credit endpoints return TransactionData bare, with no
        // `transactionData`/`estimatedSolFee` wrapper.
        let json = serde_json::json!({
            "transactions": [{ "serializedTransaction": "AQID" }],
            "parallel": false,
            "tag": "dc_delegate_abc",
            "actionMetadata": { "type": "dc_delegate" }
        });
        let data: TransactionData =
            serde_json::from_value(json).expect("deserialize bare TransactionData");
        assert_eq!(data.transactions.len(), 1);
        assert_eq!(data.tag.as_deref(), Some("dc_delegate_abc"));
        assert!(ApiTransactions::estimated_sol_fee(&data).is_none());
        assert!(std::ptr::eq(
            ApiTransactions::transaction_data(&data),
            &data
        ));
    }

    #[test]
    fn transfer_request_serializes_camel_case() {
        let mint = Pubkey::new_unique();
        let req = TokenTransferRequest {
            wallet_address: "wallet".to_string(),
            destination: "dest".to_string(),
            token_amount: TokenAmountInput::new(&mint, 100_000_000),
            multisig: None,
            memo: None,
        };
        let v = serde_json::to_value(&req).expect("serialize request");
        assert_eq!(v["walletAddress"], "wallet");
        assert_eq!(v["tokenAmount"]["amount"], "100000000");
        assert_eq!(v["tokenAmount"]["mint"], mint.to_string());
        // Omitted Squads fields must not appear on a direct transfer.
        assert!(v.get("multisig").is_none());
        assert!(v.get("memo").is_none());
    }

    #[test]
    fn squads_config_action_serializes_tagged_camel_case() {
        let add = SquadsConfigAction::AddMember {
            new_member: "m".to_string(),
            permissions: Some(vec![SquadsPermission::Initiate, SquadsPermission::Vote]),
        };
        let v = serde_json::to_value(&add).expect("serialize add member");
        assert_eq!(v["type"], "addMember");
        assert_eq!(v["newMember"], "m");
        assert_eq!(v["permissions"][0], "initiate");
        assert_eq!(v["permissions"][1], "vote");

        let threshold = SquadsConfigAction::ChangeThreshold { new_threshold: 2 };
        let tv = serde_json::to_value(&threshold).expect("serialize threshold");
        assert_eq!(tv["type"], "changeThreshold");
        assert_eq!(tv["newThreshold"], 2);

        let vote = SquadsProposalVoteRequest {
            member: "mem".to_string(),
            multisig: "ms".to_string(),
            transaction_index: "7".to_string(),
            memo: None,
        };
        let vv = serde_json::to_value(&vote).expect("serialize vote");
        assert_eq!(vv["member"], "mem");
        assert_eq!(vv["transactionIndex"], "7");
        assert!(vv.get("memo").is_none());
    }

    #[test]
    fn wifi_deployment_info_serializes_tagged_camel_case() {
        let info = DeploymentInfo::Wifi {
            antenna: Some(3),
            elevation: Some(5.0),
            azimuth: None,
            mechanical_down_tilt: Some(1.5),
            electrical_down_tilt: None,
            serial: None,
        };
        let v = serde_json::to_value(&info).expect("serialize deployment info");
        assert_eq!(v["type"], "WIFI");
        assert_eq!(v["antenna"], 3);
        assert_eq!(v["mechanicalDownTilt"], 1.5);
        // Absent fields omitted so the server keeps current values.
        assert!(v.get("azimuth").is_none());
        assert!(v.get("serial").is_none());
    }

    #[test]
    fn dc_mint_omits_absent_fields() {
        let req = DcMintRequest {
            owner: "owner".to_string(),
            dc_amount: Some("100".to_string()),
            hnt_amount: None,
            recipient: None,
        };
        let v = serde_json::to_value(&req).expect("serialize request");
        assert_eq!(v["dcAmount"], "100");
        assert!(v.get("hntAmount").is_none());
        assert!(v.get("recipient").is_none());
    }

    #[test]
    fn swap_quote_round_trips_including_extra_fields() {
        // The quote must re-serialize verbatim (routePlan, platformFee, …) so it
        // can be sent back as `quoteResponse` to swap/instructions.
        let json = serde_json::json!({
            "inputMint": "hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux",
            "inAmount": "100000000",
            "outputMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "outAmount": "450000",
            "otherAmountThreshold": "445500",
            "swapMode": "ExactIn",
            "slippageBps": 100,
            "priceImpactPct": "0.01",
            "routePlan": [{ "swapInfo": { "label": "Orca" }, "percent": 100 }],
            "platformFee": null,
            "contextSlot": 123456
        });
        let quote: SwapQuote =
            serde_json::from_value(json.clone()).expect("deserialize swap quote");
        assert_eq!(quote.out_amount, "450000");
        assert_eq!(quote.price_impact_pct, "0.01");
        // Re-serializing reproduces every field, including those in `extra`.
        assert_eq!(serde_json::to_value(&quote).expect("serialize quote"), json);
    }

    #[test]
    fn new_action_requests_serialize_camel_case() {
        let mint = Pubkey::new_unique();
        let burn = TokenBurnRequest {
            wallet_address: "w".to_string(),
            token_amount: TokenAmountInput::new(&mint, 5),
            multisig: None,
            memo: None,
        };
        let burn_json = serde_json::to_value(&burn).expect("serialize burn");
        assert_eq!(burn_json["walletAddress"], "w");
        // Omitted propose fields must not appear on a direct burn.
        assert!(burn_json.get("multisig").is_none());

        let dc = DcBurnRequest {
            owner: "o".to_string(),
            amount: "3".to_string(),
            multisig: Some("ms".to_string()),
            memo: None,
        };
        let dc_json = serde_json::to_value(&dc).expect("serialize dc burn");
        assert_eq!(dc_json["amount"], "3");
        assert_eq!(dc_json["multisig"], "ms");

        let hotspot = HotspotBurnRequest {
            wallet_address: "w".to_string(),
            hotspot_pubkey: "h".to_string(),
            multisig: None,
            memo: None,
        };
        assert_eq!(
            serde_json::to_value(&hotspot).expect("serialize hotspot burn")["hotspotPubkey"],
            "h"
        );

        let claim = ClaimHotspotRewardsRequest {
            wallet_address: "w".to_string(),
            network: Some(RewardNetwork::Iot),
        };
        let claim_json = serde_json::to_value(&claim).expect("serialize claim");
        assert_eq!(claim_json["walletAddress"], "w");
        assert_eq!(claim_json["network"], "iot");

        let memo = MemoRequest {
            wallet_address: "w".to_string(),
            memo: "hi".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&memo).expect("serialize memo")["memo"],
            "hi"
        );
    }

    #[test]
    fn automation_requests_send_schedule_and_omit_the_path_wallet() {
        // The wallet is the URL's path parameter, so it must not appear in the
        // body; `schedule` is the server's field name for the cadence.
        let setup = SetupAutomationRequest {
            wallet_address: "w".to_string(),
            schedule: "0 0 0 1 * * *".to_string(),
            duration: 30,
        };
        let v = serde_json::to_value(&setup).expect("serialize setup");
        assert_eq!(v["schedule"], "0 0 0 1 * * *");
        assert_eq!(v["duration"], 30);
        assert!(v.get("cronSchedule").is_none());
        assert!(v.get("walletAddress").is_none());

        let fund = FundAutomationRequest {
            wallet_address: "w".to_string(),
            additional_duration: 5,
        };
        let fv = serde_json::to_value(&fund).expect("serialize fund");
        assert_eq!(fv["additionalDuration"], 5);
        assert!(fv.get("walletAddress").is_none());

        let add_entity = AddEntityToAutomationRequest {
            wallet_address: "w".to_string(),
            entity_key: "e".to_string(),
        };
        let av = serde_json::to_value(&add_entity).expect("serialize add entity");
        assert_eq!(av["entityKey"], "e");
        assert!(av.get("walletAddress").is_none());

        let remove_entity = RemoveEntityFromAutomationRequest {
            wallet_address: "w".to_string(),
            index: 2,
        };
        let rv = serde_json::to_value(&remove_entity).expect("serialize remove entity");
        assert_eq!(rv["index"], 2);
        assert!(rv.get("walletAddress").is_none());

        // The wallet-only bodies carry nothing at all.
        for body in [
            serde_json::to_value(CloseAutomationRequest {
                wallet_address: "w".to_string(),
            })
            .expect("serialize close"),
            serde_json::to_value(RequeueAutomationRequest {
                wallet_address: "w".to_string(),
            })
            .expect("serialize requeue"),
            serde_json::to_value(AddWalletToAutomationRequest {
                wallet_address: "w".to_string(),
            })
            .expect("serialize add wallet"),
        ] {
            assert_eq!(body, serde_json::json!({}));
        }
    }

    #[test]
    fn batch_status_terminal_and_success() {
        assert!(!BatchStatus::Pending.is_terminal());
        assert!(BatchStatus::Confirmed.is_terminal());
        assert!(BatchStatus::Confirmed.is_success());
        assert!(BatchStatus::Failed.is_terminal());
        assert!(!BatchStatus::Failed.is_success());
        assert!(!BatchStatus::Partial.is_success());
    }

    fn sample_tx() -> VersionedTransaction {
        // A minimal, unsigned single-account legacy message wrapped versioned.
        let msg = VersionedMessage::Legacy(Message {
            header: MessageHeader {
                num_required_signatures: 0,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            account_keys: vec![Pubkey::new_unique()],
            recent_blockhash: solana_sdk::hash::Hash::default(),
            instructions: vec![],
        });
        VersionedTransaction {
            signatures: vec![],
            message: msg,
        }
    }

    #[test]
    fn transaction_base64_round_trips() {
        let tx = sample_tx();
        let encoded = encode_transaction(&tx).expect("encode tx");
        let decoded = decode_transaction(&encoded).expect("decode tx");
        assert_eq!(decoded.message, tx.message);
    }

    #[test]
    fn decodes_live_server_transaction() {
        // A real unsigned v0 transaction captured from
        // POST /api/v1/tokens/transfer on the live blockchain-api. Locks the
        // base64+bincode decode path against actual server output (web3.js
        // `VersionedTransaction.serialize()`).
        const LIVE_TX: &str = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAFBwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABXJTo3ilnlBqqNoGd80yKLSQdw4mFADklcXlEbJt3GgcDBkZv5SEXMv/srbpyw5vnvIzlu8X3EmssQ5s6QAAAAIyXJY9OJInxuz0QKRSODYMLWhOZ2v8QhASOe9jb6fhZCnMgk5GFYffdf8vsSr2FE97KGpZ/etejnWO0HtiTgIsAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAbd9uHXZaGT2cvhRs7reawctIXtX1s3kTqM9YV+/wCpwKblKrctuLwb91a9Vzh3AR7r2HJaVr5NHVH+MhVL//0EAgAFAroUAAACAAkDECcAAAAAAAADBgABAAQFBgEBBgQBBAEACgwBAAAAAAAAAAgA";
        let tx = decode_transaction(LIVE_TX).expect("decode live server tx");
        // The server builds v0 versioned transactions with a Helium LUT.
        assert!(
            matches!(tx.message, VersionedMessage::V0(_)),
            "expected a v0 versioned message"
        );
        assert!(
            !tx.message.instructions().is_empty(),
            "expected at least one instruction"
        );
    }

    #[test]
    fn to_submit_request_preserves_envelope_and_metadata() {
        let data = TransactionData {
            transactions: vec![TransactionItem {
                serialized_transaction: "ignored".to_string(),
                metadata: Some(serde_json::json!({ "type": "t", "description": "d" })),
            }],
            parallel: true,
            tag: Some("tag1".to_string()),
            action_metadata: Some(serde_json::json!({ "k": "v" })),
        };
        let signed = vec![sample_tx()];
        let req = data
            .to_submit_request(&signed, true)
            .expect("build submit request");
        assert!(req.parallel);
        assert_eq!(req.tag.as_deref(), Some("tag1"));
        assert_eq!(req.simulate, Some(true));
        assert_eq!(req.transactions.len(), 1);
        assert_eq!(
            req.transactions[0].metadata,
            Some(serde_json::json!({ "type": "t", "description": "d" }))
        );
    }

    #[test]
    fn to_submit_request_rejects_count_mismatch() {
        let data = TransactionData {
            transactions: vec![TransactionItem {
                serialized_transaction: "a".to_string(),
                metadata: None,
            }],
            parallel: false,
            tag: None,
            action_metadata: None,
        };
        // Two signed for one returned → error.
        let signed = vec![sample_tx(), sample_tx()];
        assert!(data.to_submit_request(&signed, true).is_err());
    }
}
