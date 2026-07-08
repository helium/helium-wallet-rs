//! HTTP client for the Helium blockchain-api REST surface (`/api/v1`).
//!
//! The blockchain-api (in the HPL repo, `packages/blockchain-api`) builds
//! Solana transactions server-side and returns them **unsigned** and base64
//! encoded. This client posts an action request (with the wallet pubkey as an
//! explicit field), receives an [`ActionResponse`], and — after the caller
//! signs the decoded transactions locally — submits them back via
//! [`Client::submit`] and polls [`Client::poll_status`].
//!
//! The entire `/api/v1` surface is public (no auth). Priority fee, compute
//! budget, and lookup tables are chosen server-side and are not tunable here.
//!
//! ```no_run
//! # async fn run() -> Result<(), helium_lib::error::Error> {
//! use helium_lib::blockchain_api::{Client, types::{TokenTransferRequest, TokenAmountInput}};
//! # let mint = helium_lib::keypair::Pubkey::new_unique();
//! let client = Client::from_env()?;
//! let resp = client
//!     .token_transfer(&TokenTransferRequest {
//!         wallet_address: "…".to_string(),
//!         destination: "…".to_string(),
//!         token_amount: TokenAmountInput::new(&mint, 100_000_000),
//!         multisig: None,
//!         memo: None,
//!     })
//!     .await?;
//! let unsigned = resp.decode_transactions()?; // sign these locally, then submit
//! # let _ = unsigned;
//! # Ok(())
//! # }
//! ```

pub mod types;

use crate::{
    error::DecodeError, error::EncodeError, keypair::Pubkey, transaction::VersionedTransaction,
};
use serde::{de::DeserializeOwned, Serialize};
use std::time::{Duration, Instant};
use types::{
    ActionResponse, AddEntityToAutomationRequest, AddWalletToAutomationRequest,
    ClaimHotspotRewardsRequest, ClaimRewardsRequest, CloseAutomationRequest, DcBurnRequest,
    DcDelegateRequest, DcMintRequest, FundAutomationRequest, HotspotBurnRequest,
    IssueDataOnlyHotspotRequest, MemoRequest, MultiTransferRequest, OnboardDataOnlyHotspotRequest,
    RemoveEntityFromAutomationRequest, RequeueAutomationRequest, SetupAutomationRequest,
    SquadsExecuteProposalRequest, SquadsProposalVoteRequest, SquadsProposeConfigChangeRequest,
    StatusResponse, SubmitRequest, SubmitResponse, SwapInstructionsRequest, SwapQuote,
    TokenBurnRequest, TokenTransferRequest, TopUpAutomationRequest, TransactionData,
    TransferHotspotRequest, UpdateInfoRequest, UpdateRewardsDestinationRequest,
};

/// Environment variable holding the blockchain-api base URL (`…/api/v1`).
pub const API_URL_ENV: &str = "HELIUM_BLOCKCHAIN_API_URL";
/// Default interval between batch-status polls.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_ERROR_BODY_LEN: usize = 300;

/// Errors from the blockchain-api client.
#[derive(Debug, thiserror::Error)]
pub enum BlockchainApiError {
    /// The HTTP request itself failed (connect, TLS, timeout, body read).
    #[error("blockchain-api request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The API returned a non-2xx status.
    #[error("blockchain-api error (HTTP {status}): {message}")]
    Api { status: u16, message: String },
    /// Failed to base64/bincode-decode a returned transaction.
    #[error("decode: {0}")]
    Decode(#[from] DecodeError),
    /// Failed to serialize a signed transaction for submission.
    #[error("encode: {0}")]
    Encode(#[from] EncodeError),
    /// Client misconfiguration (missing URL, count mismatch, etc.).
    #[error("blockchain-api configuration error: {0}")]
    Config(String),
    /// Polling exceeded the deadline before the batch reached a terminal state.
    /// Carries the batch id so the caller can query it later to see if it landed.
    #[error("timed out after {timeout:?} polling blockchain-api batch {batch_id}")]
    Timeout { timeout: Duration, batch_id: String },
}

impl BlockchainApiError {
    /// Construct a configuration error.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    fn api(status: u16, body: String) -> Self {
        let message = if body.is_empty() {
            format!("empty response (HTTP {status})")
        } else if body.chars().count() > MAX_ERROR_BODY_LEN {
            // Truncate by characters, not bytes, so a multi-byte char straddling
            // the cap can't panic the whole process on an error path.
            let truncated: String = body.chars().take(MAX_ERROR_BODY_LEN).collect();
            format!("{truncated}…")
        } else {
            body
        };
        Self::Api { status, message }
    }
}

/// Client for the Helium blockchain-api REST surface.
#[derive(Clone, Debug)]
pub struct Client {
    client: reqwest::Client,
    /// Base URL up to and including `/api/v1`, without a trailing slash.
    base_url: String,
}

impl Client {
    /// Create a client for the given base URL (e.g.
    /// `https://my-helium.web.helium.io/api/v1`). A trailing slash is trimmed.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Create a client from the [`API_URL_ENV`] environment variable.
    pub fn from_env() -> Result<Self, BlockchainApiError> {
        let base_url = std::env::var(API_URL_ENV)
            .map_err(|_| BlockchainApiError::config(format!("{API_URL_ENV} not set")))?;
        Ok(Self::new(base_url))
    }

    /// The configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn post<B, R>(&self, path: &str, body: &B) -> Result<R, BlockchainApiError>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let resp = self
            .client
            .post(format!("{}{path}", self.base_url))
            .json(body)
            .send()
            .await?;
        parse(resp).await
    }

    async fn get_query<Q, R>(&self, path: &str, query: &Q) -> Result<R, BlockchainApiError>
    where
        Q: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let resp = self
            .client
            .get(format!("{}{path}", self.base_url))
            .query(query)
            .send()
            .await?;
        parse(resp).await
    }

    // ---- Action endpoints (return unsigned transactions) ----

    /// `POST /tokens/transfer` — single-recipient SPL transfer.
    pub async fn token_transfer(
        &self,
        req: &TokenTransferRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/tokens/transfer", req).await
    }

    /// `POST /tokens/multi-transfer` — many recipients of the same mint.
    pub async fn multi_transfer(
        &self,
        req: &MultiTransferRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/tokens/multi-transfer", req).await
    }

    /// `POST /data-credits/mint` — mint DC by burning HNT.
    ///
    /// The data-credit endpoints return the [`TransactionData`] bare (no
    /// `estimatedSolFee` wrapper), unlike the token/hotspot endpoints.
    pub async fn dc_mint(
        &self,
        req: &DcMintRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/data-credits/mint", req).await
    }

    /// `POST /data-credits/delegate` — delegate DC to a router.
    pub async fn dc_delegate(
        &self,
        req: &DcDelegateRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/data-credits/delegate", req).await
    }

    /// `POST /hotspots/claim-rewards`.
    pub async fn claim_rewards(
        &self,
        req: &ClaimRewardsRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/hotspots/claim-rewards", req).await
    }

    /// `POST /hotspots/update-rewards-destination`.
    pub async fn update_rewards_destination(
        &self,
        req: &UpdateRewardsDestinationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/hotspots/update-rewards-destination", req).await
    }

    /// `POST /hotspots/update-info` — assert location/antenna details.
    pub async fn update_info(
        &self,
        req: &UpdateInfoRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/hotspots/update-info", req).await
    }

    /// `POST /tokens/burn` — burn SPL tokens.
    pub async fn token_burn(
        &self,
        req: &TokenBurnRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/tokens/burn", req).await
    }

    /// `POST /tokens/memo` — emit a memo transaction.
    pub async fn memo(&self, req: &MemoRequest) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/tokens/memo", req).await
    }

    /// `POST /data-credits/burn` — burn DC. Returns the bare [`TransactionData`].
    pub async fn dc_burn(
        &self,
        req: &DcBurnRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/data-credits/burn", req).await
    }

    /// `POST /hotspots/burn` — permanently burn a hotspot cNFT.
    pub async fn burn_hotspot(
        &self,
        req: &HotspotBurnRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/hotspots/burn", req).await
    }

    /// `POST /hotspots/transfer` — transfer a hotspot cNFT to a new owner.
    pub async fn transfer_hotspot(
        &self,
        req: &TransferHotspotRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/hotspots/transfer", req).await
    }

    /// `POST /hotspots/{entity_pub_key}/claim-rewards` — claim the full pending
    /// rewards for a single hotspot (the entity key is a path parameter).
    pub async fn claim_hotspot_rewards(
        &self,
        entity_pub_key: &str,
        req: &ClaimHotspotRewardsRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post(&format!("/hotspots/{entity_pub_key}/claim-rewards"), req)
            .await
    }

    // ---- Squads v4 proposal lifecycle (return the bare TransactionData) ----

    /// `POST /squads/proposals/approve` — approve a proposal as `member`.
    pub async fn approve_proposal(
        &self,
        req: &SquadsProposalVoteRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/squads/proposals/approve", req).await
    }

    /// `POST /squads/proposals/reject` — reject a proposal as `member`.
    pub async fn reject_proposal(
        &self,
        req: &SquadsProposalVoteRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/squads/proposals/reject", req).await
    }

    /// `POST /squads/proposals/cancel` — cancel an approved proposal as `member`.
    pub async fn cancel_proposal(
        &self,
        req: &SquadsProposalVoteRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/squads/proposals/cancel", req).await
    }

    /// `POST /squads/proposals/execute` — execute an approved proposal (vault or
    /// config; the server detects which).
    pub async fn execute_proposal(
        &self,
        req: &SquadsExecuteProposalRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/squads/proposals/execute", req).await
    }

    /// `POST /squads/proposals/config` — propose a config change (add/remove
    /// member, change threshold). The assigned proposal index is returned in the
    /// response's `actionMetadata.transactionIndex`.
    pub async fn propose_config_change(
        &self,
        req: &SquadsProposeConfigChangeRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/squads/proposals/config", req).await
    }

    /// `POST /hotspots/data-only/issue` — issue (mint) a data-only hotspot. The
    /// onboarding server co-signs against the ECC verifier; the returned tx
    /// needs only the owner signature.
    pub async fn issue_data_only_hotspot(
        &self,
        req: &IssueDataOnlyHotspotRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/hotspots/data-only/issue", req).await
    }

    /// `POST /hotspots/data-only/onboard` — onboard a data-only hotspot into a
    /// sub-DAO, asserting location (and, for IoT, gain/elevation).
    pub async fn onboard_data_only_hotspot(
        &self,
        req: &OnboardDataOnlyHotspotRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/hotspots/data-only/onboard", req).await
    }

    // ---- Claim automation (one tuktuk claim cron per wallet) ----
    //
    // All return the `{transactionData, estimatedSolFee}` envelope. The cron is
    // set up empty, then whole-wallet and/or per-hotspot claims are attached to
    // it. Funding is by claim-cycle count (`duration`) across both pools (the
    // cron-crank pool and the claim-payer pool).

    /// `POST /hotspots/wallet/{wallet}/automation` — set up the claim cron on a
    /// raw crontab schedule, pre-funded for `duration` claim cycles.
    pub async fn setup_automation(
        &self,
        req: &SetupAutomationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post(
            &format!("/hotspots/wallet/{}/automation", req.wallet_address),
            req,
        )
        .await
    }

    /// `POST /hotspots/wallet/{wallet}/automation/fund` — fund `additional_duration`
    /// more claim cycles across both pools.
    pub async fn fund_automation(
        &self,
        req: &FundAutomationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post(
            &format!("/hotspots/wallet/{}/automation/fund", req.wallet_address),
            req,
        )
        .await
    }

    /// `POST /hotspots/wallet/{wallet}/automation/close` — remove all claims and
    /// close the cron, refunding rent.
    pub async fn close_automation(
        &self,
        req: &CloseAutomationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post(
            &format!("/hotspots/wallet/{}/automation/close", req.wallet_address),
            req,
        )
        .await
    }

    /// `POST /hotspots/wallet/{wallet}/automation/requeue` — requeue a cron that
    /// ran out of SOL (fund it first).
    pub async fn requeue_automation(
        &self,
        req: &RequeueAutomationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post(
            &format!("/hotspots/wallet/{}/automation/requeue", req.wallet_address),
            req,
        )
        .await
    }

    /// `POST /hotspots/wallet/{wallet}/automation/add-wallet` — add a whole-wallet
    /// claim (claims every hotspot the wallet owns) to the cron.
    pub async fn add_wallet_to_automation(
        &self,
        req: &AddWalletToAutomationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post(
            &format!(
                "/hotspots/wallet/{}/automation/add-wallet",
                req.wallet_address
            ),
            req,
        )
        .await
    }

    /// `POST /hotspots/wallet/{wallet}/automation/add-entity` — add a single
    /// hotspot's claim to the cron.
    pub async fn add_entity_to_automation(
        &self,
        req: &AddEntityToAutomationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post(
            &format!(
                "/hotspots/wallet/{}/automation/add-entity",
                req.wallet_address
            ),
            req,
        )
        .await
    }

    /// `POST /hotspots/wallet/{wallet}/automation/remove-entity` — remove a single
    /// claim entry from the cron by its transaction index.
    pub async fn remove_entity_from_automation(
        &self,
        req: &RemoveEntityFromAutomationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post(
            &format!(
                "/hotspots/wallet/{}/automation/remove-entity",
                req.wallet_address
            ),
            req,
        )
        .await
    }

    /// `POST /hotspots/automation/top-up` — operator floor top-up of a batch of
    /// wallets' claim-cron pools (the operator signs and funds).
    pub async fn top_up_automation(
        &self,
        req: &TopUpAutomationRequest,
    ) -> Result<ActionResponse, BlockchainApiError> {
        self.post("/hotspots/automation/top-up", req).await
    }

    /// `GET /swap/quote` — a Jupiter-backed quote for `amount` of `input_mint`
    /// into `output_mint`. Pass the result back to [`Self::swap_instructions`].
    pub async fn swap_quote(
        &self,
        input_mint: &Pubkey,
        output_mint: &Pubkey,
        amount: u64,
        slippage_bps: u16,
    ) -> Result<SwapQuote, BlockchainApiError> {
        self.get_query(
            "/swap/quote",
            &[
                ("inputMint", input_mint.to_string()),
                ("outputMint", output_mint.to_string()),
                ("amount", amount.to_string()),
                ("slippageBps", slippage_bps.to_string()),
            ],
        )
        .await
    }

    /// `POST /swap/instructions` — build the swap transaction for a quote.
    /// Returns the bare [`TransactionData`] (no fee estimate), like the
    /// data-credit endpoints.
    pub async fn swap_instructions(
        &self,
        req: &SwapInstructionsRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.post("/swap/instructions", req).await
    }

    // ---- Submission + status ----

    /// `POST /transactions` — broadcast signed transactions as a batch.
    pub async fn submit(&self, req: &SubmitRequest) -> Result<SubmitResponse, BlockchainApiError> {
        self.post("/transactions", req).await
    }

    /// Sign-and-forget convenience: build the submit request from an action's
    /// [`TransactionData`] plus the locally-signed transactions, then submit.
    pub async fn submit_signed(
        &self,
        data: &TransactionData,
        signed: &[VersionedTransaction],
        simulate: bool,
    ) -> Result<SubmitResponse, BlockchainApiError> {
        let req = data.to_submit_request(signed, simulate)?;
        self.submit(&req).await
    }

    /// `GET /transactions/{id}` — current status of a submitted batch. The
    /// contract requires a `commitment` query param; we poll at `confirmed`.
    pub async fn status(&self, batch_id: &str) -> Result<StatusResponse, BlockchainApiError> {
        self.get_query(
            &format!("/transactions/{batch_id}"),
            &[("commitment", "confirmed")],
        )
        .await
    }

    /// Poll batch status until it reaches a terminal state or `timeout` elapses.
    pub async fn poll_status(
        &self,
        batch_id: &str,
        interval: Duration,
        timeout: Duration,
    ) -> Result<StatusResponse, BlockchainApiError> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = self.status(batch_id).await?;
            if status.status.is_terminal() {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(BlockchainApiError::Timeout {
                    timeout,
                    batch_id: batch_id.to_string(),
                });
            }
            futures_timer::Delay::new(interval).await;
        }
    }
}

async fn parse<R: DeserializeOwned>(resp: reqwest::Response) -> Result<R, BlockchainApiError> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        Ok(resp.json::<R>().await?)
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(BlockchainApiError::api(status, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trims_trailing_slash() {
        let c = Client::new("https://example.com/api/v1/");
        assert_eq!(c.base_url(), "https://example.com/api/v1");
    }

    #[test]
    fn from_env_errors_when_unset() {
        // Snapshot + clear to avoid clobbering a real value.
        let prev = std::env::var(API_URL_ENV).ok();
        std::env::remove_var(API_URL_ENV);
        assert!(matches!(
            Client::from_env(),
            Err(BlockchainApiError::Config(_))
        ));
        if let Some(v) = prev {
            std::env::set_var(API_URL_ENV, v);
        }
    }

    #[test]
    fn api_error_truncates_long_body() {
        let err = BlockchainApiError::api(500, "x".repeat(1000));
        match err {
            BlockchainApiError::Api { message, .. } => {
                assert!(message.len() <= MAX_ERROR_BODY_LEN + "…".len());
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }
}
