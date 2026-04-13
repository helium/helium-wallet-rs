use crate::{
    client::SolanaRpcClient,
    keypair::{Keypair, Pubkey},
    transaction::VersionedTransaction,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use solana_sdk::signer::Signer;

/// Default Jupiter V2 swap API base URL.
pub const DEFAULT_API_URL: &str = "https://api.jup.ag/swap/v2";
/// Default slippage tolerance in basis points (100 bps = 1%).
pub const DEFAULT_SLIPPAGE_BPS: u16 = 100;
const MAX_ERROR_BODY_LEN: usize = 200;

/// Errors that can occur during Jupiter swap operations.
#[derive(Debug, thiserror::Error)]
pub enum JupiterError {
    /// HTTP request to Jupiter API failed.
    #[error("Jupiter API request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// Jupiter API returned a non-200 status code.
    #[error("Jupiter API error (HTTP {status}): {message}")]
    Api { status: u16, message: String },
    /// Jupiter reported an error within the swap response body.
    #[error("Jupiter swap error: {0}")]
    SwapError(String),
    /// No swap routes found for the given token pair.
    #[error("Jupiter quote returned no routes for {input_mint} → {output_mint}")]
    NoRoutes {
        input_mint: String,
        output_mint: String,
    },
    /// Failed to deserialize the base64-encoded swap transaction.
    #[error("Failed to decode swap transaction: {0}")]
    TransactionDecode(String),
    /// Solana RPC call failed during swap execution.
    #[error("Solana RPC error: {0}")]
    Solana(String),
    /// Transaction signing failed.
    #[error("Transaction signing failed: {0}")]
    Signing(String),
    /// Invalid client configuration (e.g., bad slippage value).
    #[error("Jupiter configuration error: {0}")]
    Config(String),
}

impl JupiterError {
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn transaction_decode(msg: impl std::fmt::Display) -> Self {
        Self::TransactionDecode(msg.to_string())
    }

    pub fn solana(msg: impl std::fmt::Display) -> Self {
        Self::Solana(msg.to_string())
    }

    pub fn signing(msg: impl std::fmt::Display) -> Self {
        Self::Signing(msg.to_string())
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

/// Jupiter swap API client (V2).
///
/// Works with or without an API key:
/// - Without key: keyless access at 0.5 RPS (suitable for CLI use)
/// - With `JUP_API_KEY`: higher rate limits per your plan (required for automated/production use)
///
/// NOTE: intentionally no `Debug` derive — `api_key` is a secret.
#[derive(Clone)]
pub struct Client {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    slippage_bps: u16,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field(
                "api_key",
                &if self.api_key.is_some() {
                    "[redacted]"
                } else {
                    "[none]"
                },
            )
            .field("slippage_bps", &self.slippage_bps)
            .finish()
    }
}

impl Client {
    /// Creates a new Jupiter client with explicit configuration.
    pub fn new(
        api_key: Option<impl Into<String>>,
        base_url: impl Into<String>,
        slippage_bps: u16,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key: api_key.map(Into::into),
            slippage_bps,
        }
    }

    /// Create a client from environment variables.
    ///
    /// `JUP_API_KEY` is optional — omit for keyless access (0.5 RPS).
    /// `JUP_API_URL` defaults to `https://api.jup.ag/swap/v2`.
    /// `JUP_SLIPPAGE_BPS` defaults to 100 (1%).
    pub fn from_env() -> Result<Self, JupiterError> {
        let api_key = std::env::var("JUP_API_KEY").ok();
        let base_url = std::env::var("JUP_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string());
        let slippage_bps = match std::env::var("JUP_SLIPPAGE_BPS") {
            Ok(v) => v.parse::<u16>().map_err(|_| {
                JupiterError::config(format!("JUP_SLIPPAGE_BPS={v:?} is not a valid u16"))
            })?,
            Err(_) => DEFAULT_SLIPPAGE_BPS,
        };
        Ok(Self::new(api_key, base_url, slippage_bps))
    }

    /// Whether this client has an API key configured.
    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => req.header("x-api-key", key),
            None => req,
        }
    }

    /// Get a swap order from Jupiter: quote + assembled transaction in one call.
    ///
    /// Returns an `OrderResponse` containing the quote data and a base64-encoded
    /// transaction ready to be decoded, signed, and sent.
    ///
    /// The `taker` is the wallet public key that will sign and pay for the swap.
    pub async fn order(
        &self,
        input_mint: &Pubkey,
        output_mint: &Pubkey,
        amount: u64,
        taker: &Pubkey,
    ) -> Result<OrderResponse, JupiterError> {
        if amount == 0 {
            return Err(JupiterError::config(
                "swap amount must be greater than zero",
            ));
        }
        if input_mint == output_mint {
            return Err(JupiterError::config(
                "input and output tokens must be different",
            ));
        }

        let url = format!("{}/order", self.base_url);
        let req = self.client.get(&url).query(&[
            ("inputMint", input_mint.to_string()),
            ("outputMint", output_mint.to_string()),
            ("amount", amount.to_string()),
            ("slippageBps", self.slippage_bps.to_string()),
            ("taker", taker.to_string()),
        ]);
        let resp = self.apply_auth(req).send().await?;

        match resp.status().as_u16() {
            200 => {
                let order: OrderResponse = resp.json().await?;
                // Check for inline error from Jupiter
                if let Some(error_msg) = &order.error_message {
                    return Err(JupiterError::SwapError(error_msg.clone()));
                }
                if order.route_plan.is_empty() {
                    return Err(JupiterError::NoRoutes {
                        input_mint: input_mint.to_string(),
                        output_mint: output_mint.to_string(),
                    });
                }
                Ok(order)
            }
            status => {
                let body = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".to_string());
                Err(JupiterError::api(status, body))
            }
        }
    }

    /// Get a swap order and build a signed transaction ready to send.
    ///
    /// Combines `order()` with transaction decoding, blockhash update, and signing.
    /// Returns the signed transaction, last valid block height, and the order
    /// response (for quote data like amounts, rate, price impact).
    ///
    /// Note: the transaction's compute budget is set by Jupiter's simulation —
    /// `TransactionOpts` is intentionally not accepted here.
    pub async fn swap<C: AsRef<SolanaRpcClient>>(
        &self,
        client: &C,
        input_mint: &Pubkey,
        output_mint: &Pubkey,
        amount: u64,
        keypair: &Keypair,
    ) -> Result<(VersionedTransaction, u64, OrderResponse), JupiterError> {
        let order = self
            .order(input_mint, output_mint, amount, &keypair.pubkey())
            .await?;

        if order.transaction.is_empty() {
            return Err(JupiterError::SwapError(
                order
                    .error_message
                    .unwrap_or_else(|| "no transaction returned".to_string()),
            ));
        }

        // Decode base64 → bincode → VersionedTransaction
        let tx_bytes = BASE64
            .decode(&order.transaction)
            .map_err(JupiterError::transaction_decode)?;
        let mut txn: VersionedTransaction =
            bincode::deserialize(&tx_bytes).map_err(JupiterError::transaction_decode)?;

        // Update to a fresh blockhash and re-sign
        let solana_client = client.as_ref();
        let (blockhash, last_valid_block_height) = solana_client
            .get_latest_blockhash_with_commitment(solana_client.commitment())
            .await
            .map_err(JupiterError::solana)?;
        txn.message.set_recent_blockhash(blockhash);
        let txn = VersionedTransaction::try_new(txn.message, &[keypair])
            .map_err(JupiterError::signing)?;

        Ok((txn, last_valid_block_height, order))
    }
}

// ---- Jupiter API response types ----

/// Response from the Jupiter V2 `/order` endpoint.
///
/// Contains both the quote data (amounts, route, price impact) and the
/// assembled transaction. The transaction may be empty if there was an
/// error (check `error_message`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderResponse {
    /// Input token mint address.
    pub input_mint: String,
    /// Output token mint address.
    pub output_mint: String,
    /// Input amount in smallest token units.
    pub in_amount: String,
    /// Quoted output amount in smallest token units.
    pub out_amount: String,
    /// Estimated price impact as a percentage string.
    pub price_impact_pct: String,
    /// Slippage tolerance used for this order.
    pub slippage_bps: u16,
    #[serde(default)]
    pub(crate) route_plan: Vec<RoutePlan>,
    #[serde(default)]
    pub transaction: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_code: Option<i32>,
    #[serde(default)]
    pub error_message: Option<String>,
    /// All other fields captured for forward compatibility.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutePlan {
    #[serde(default)]
    pub swap_info: SwapInfo,
    #[serde(default)]
    pub percent: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwapInfo {
    pub label: Option<String>,
    pub input_mint: Option<String>,
    pub output_mint: Option<String>,
    pub in_amount: Option<String>,
    pub out_amount: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutex to serialize env-dependent tests (env vars are process-global).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clean_env() {
        std::env::remove_var("JUP_API_KEY");
        std::env::remove_var("JUP_API_URL");
        std::env::remove_var("JUP_SLIPPAGE_BPS");
    }

    #[test]
    fn test_from_env_no_api_key_succeeds() {
        let _lock = ENV_LOCK.lock().unwrap();
        clean_env();
        let client = Client::from_env().unwrap();
        assert!(!client.has_api_key());
    }

    #[test]
    fn test_from_env_with_api_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        clean_env();
        std::env::set_var("JUP_API_KEY", "test-key");
        let client = Client::from_env().unwrap();
        assert!(client.has_api_key());
        clean_env();
    }

    #[test]
    fn test_from_env_invalid_slippage() {
        let _lock = ENV_LOCK.lock().unwrap();
        clean_env();
        std::env::set_var("JUP_SLIPPAGE_BPS", "not-a-number");
        let result = Client::from_env();
        clean_env();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), JupiterError::Config(_)));
    }

    #[test]
    fn test_debug_redacts_api_key() {
        let client = Client::new(Some("secret-key-123"), DEFAULT_API_URL, 100);
        let debug = format!("{client:?}");
        assert!(!debug.contains("secret-key-123"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn test_debug_no_api_key() {
        let client = Client::new(None::<String>, DEFAULT_API_URL, 100);
        let debug = format!("{client:?}");
        assert!(debug.contains("[none]"));
    }

    #[test]
    fn test_error_body_truncation() {
        let long_body = "x".repeat(500);
        let err = JupiterError::api(500, long_body);
        match err {
            JupiterError::Api { message, .. } => {
                assert!(message.len() <= MAX_ERROR_BODY_LEN + "…".len());
            }
            _ => panic!("expected Api variant"),
        }
    }

    #[test]
    fn test_empty_error_body() {
        let err = JupiterError::api(404, String::new());
        match err {
            JupiterError::Api { message, .. } => {
                assert!(message.contains("empty response"));
            }
            _ => panic!("expected Api variant"),
        }
    }

    #[test]
    fn test_order_response_deserialization() {
        let json = serde_json::json!({
            "inputMint": "hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux",
            "inAmount": "100000000",
            "outputMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "outAmount": "450000",
            "otherAmountThreshold": "445500",
            "swapMode": "ExactIn",
            "slippageBps": 100,
            "priceImpactPct": "0.01",
            "transaction": "base64encodedtx",
            "routePlan": [{
                "swapInfo": {
                    "ammKey": "abc123",
                    "label": "Orca",
                    "inputMint": "hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux",
                    "outputMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    "inAmount": "100000000",
                    "outAmount": "450000"
                },
                "percent": 100
            }]
        });

        let order: OrderResponse = serde_json::from_value(json).unwrap();
        assert_eq!(order.in_amount, "100000000");
        assert_eq!(order.out_amount, "450000");
        assert_eq!(order.slippage_bps, 100);
        assert_eq!(order.transaction, "base64encodedtx");
        assert_eq!(order.route_plan.len(), 1);
        assert!((order.route_plan[0].percent - 100.0).abs() < f64::EPSILON);
        assert!(order.error_message.is_none());
    }

    #[test]
    fn test_order_response_with_error() {
        let json = serde_json::json!({
            "inputMint": "abc",
            "outputMint": "def",
            "inAmount": "100",
            "outAmount": "0",
            "priceImpactPct": "0",
            "slippageBps": 100,
            "transaction": "",
            "routePlan": [],
            "error": "Insufficient funds",
            "errorCode": 1,
            "errorMessage": "Insufficient funds"
        });

        let order: OrderResponse = serde_json::from_value(json).unwrap();
        assert_eq!(order.error_message.as_deref(), Some("Insufficient funds"));
        assert!(order.transaction.is_empty());
    }
}
