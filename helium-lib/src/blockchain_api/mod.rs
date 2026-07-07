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
    ActionResponse, ClaimHotspotRewardsRequest, ClaimRewardsRequest, DcBurnRequest,
    DcDelegateRequest, DcMintRequest, HotspotBurnRequest, MemoRequest, MultiTransferRequest,
    StatusResponse, SubmitRequest, SubmitResponse, SwapInstructionsRequest, SwapQuote,
    TokenBurnRequest, TokenTransferRequest, TransactionData, UpdateInfoRequest,
    UpdateRewardsDestinationRequest,
};

/// Environment variable holding the blockchain-api base URL (`…/api/v1`).
pub const API_URL_ENV: &str = "HELIUM_BLOCKCHAIN_API_URL";
/// Default interval between batch-status polls.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Default deadline for batch-status polling (matches the CLI's 90s default).
pub const DEFAULT_POLL_TIMEOUT: Duration = Duration::from_secs(90);
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
    #[error("timed out after {0:?} polling blockchain-api batch status")]
    Timeout(Duration),
}

impl BlockchainApiError {
    /// Construct a configuration error.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    fn api(status: u16, body: String) -> Self {
        let message = if body.is_empty() {
            format!("empty response (HTTP {status})")
        } else if body.len() > MAX_ERROR_BODY_LEN {
            format!("{}…", &body[..MAX_ERROR_BODY_LEN])
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

    async fn get<R: DeserializeOwned>(&self, path: &str) -> Result<R, BlockchainApiError> {
        let resp = self
            .client
            .get(format!("{}{path}", self.base_url))
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

    /// `GET /transactions/{id}` — current status of a submitted batch.
    pub async fn status(&self, batch_id: &str) -> Result<StatusResponse, BlockchainApiError> {
        self.get(&format!("/transactions/{batch_id}")).await
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
                return Err(BlockchainApiError::Timeout(timeout));
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
