use crate::{
    client::SolanaRpcClient,
    keypair::{Keypair, Pubkey},
    transaction::VersionedTransaction,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use solana_sdk::signer::Signer;

pub const DEFAULT_API_URL: &str = "https://api.jup.ag/swap/v6";
pub const DEFAULT_SLIPPAGE_BPS: u16 = 100;
const MAX_ERROR_BODY_LEN: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum JupiterError {
    #[error("Jupiter API request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Jupiter API error (HTTP {status}): {message}")]
    Api { status: u16, message: String },
    #[error("Jupiter quote returned no routes for {input_mint} → {output_mint}")]
    NoRoutes {
        input_mint: String,
        output_mint: String,
    },
    #[error("Failed to decode swap transaction: {0}")]
    TransactionDecode(String),
    #[error("Solana RPC error: {0}")]
    Solana(String),
    #[error("Transaction signing failed: {0}")]
    Signing(String),
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
        let message = if body.len() > MAX_ERROR_BODY_LEN {
            format!("{}…", &body[..MAX_ERROR_BODY_LEN])
        } else {
            body
        };
        Self::Api { status, message }
    }
}

/// Jupiter V6 swap API client.
///
/// NOTE: intentionally no `Debug` derive — `api_key` is a secret.
#[derive(Clone)]
pub struct Client {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    slippage_bps: u16,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("api_key", &"[redacted]")
            .field("slippage_bps", &self.slippage_bps)
            .finish()
    }
}

impl Client {
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>, slippage_bps: u16) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            slippage_bps,
        }
    }

    pub fn from_env() -> Result<Self, JupiterError> {
        let api_key = std::env::var("JUP_API_KEY")
            .map_err(|_| JupiterError::config("JUP_API_KEY not configured"))?;
        let base_url = std::env::var("JUP_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string());
        let slippage_bps = match std::env::var("JUP_SLIPPAGE_BPS") {
            Ok(v) => v.parse::<u16>().map_err(|_| {
                JupiterError::config(format!("JUP_SLIPPAGE_BPS={v:?} is not a valid u16"))
            })?,
            Err(_) => DEFAULT_SLIPPAGE_BPS,
        };
        Ok(Self::new(api_key, base_url, slippage_bps))
    }

    /// Get a swap quote from Jupiter.
    pub async fn quote(
        &self,
        input_mint: &Pubkey,
        output_mint: &Pubkey,
        amount: u64,
    ) -> Result<QuoteResponse, JupiterError> {
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

        let url = format!("{}/quote", self.base_url);
        let resp = self
            .client
            .get(&url)
            .query(&[
                ("inputMint", input_mint.to_string()),
                ("outputMint", output_mint.to_string()),
                ("amount", amount.to_string()),
                ("slippageBps", self.slippage_bps.to_string()),
            ])
            .header("x-api-key", &self.api_key)
            .send()
            .await?;

        match resp.status().as_u16() {
            200 => {
                let quote: QuoteResponse = resp.json().await?;
                if quote.route_plan.is_empty() {
                    return Err(JupiterError::NoRoutes {
                        input_mint: input_mint.to_string(),
                        output_mint: output_mint.to_string(),
                    });
                }
                Ok(quote)
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

    /// Get a swap transaction from Jupiter, deserialize it, update the
    /// blockhash, and sign with the provided keypair.
    ///
    /// Returns a ready-to-send `VersionedTransaction` and the last valid
    /// block height, matching the helium-lib convention for transaction
    /// builders (`token::transfer`, `token::burn`, etc.).
    ///
    /// Note: the transaction's compute budget is set by Jupiter's
    /// simulation — `TransactionOpts` is intentionally not accepted here,
    /// unlike other helium-lib transaction builders.
    pub async fn swap<C: AsRef<SolanaRpcClient>>(
        &self,
        client: &C,
        quote: &QuoteResponse,
        keypair: &Keypair,
    ) -> Result<(VersionedTransaction, u64), JupiterError> {
        let swap_request = SwapRequest {
            user_public_key: keypair.pubkey().to_string(),
            quote_response: quote.clone(),
            wrap_and_unwrap_sol: true,
        };

        let url = format!("{}/swap", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .json(&swap_request)
            .send()
            .await?;

        let swap_response: SwapResponse = match resp.status().as_u16() {
            200 => resp.json().await?,
            status => {
                let body = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".to_string());
                return Err(JupiterError::api(status, body));
            }
        };

        // Decode base64 → bincode → VersionedTransaction
        let tx_bytes = BASE64
            .decode(&swap_response.swap_transaction)
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

        Ok((txn, last_valid_block_height))
    }
}

// ---- Jupiter API request/response types ----

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SwapRequest {
    user_public_key: String,
    quote_response: QuoteResponse,
    wrap_and_unwrap_sol: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwapResponse {
    swap_transaction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteResponse {
    pub input_mint: String,
    pub output_mint: String,
    pub in_amount: String,
    pub out_amount: String,
    pub price_impact_pct: String,
    pub slippage_bps: u16,
    #[serde(default)]
    pub(crate) route_plan: Vec<RoutePlan>,
    /// All other fields are captured for pass-through to the /swap endpoint.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoutePlan {
    #[serde(default)]
    pub swap_info: SwapInfo,
    pub percent: u8,
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

    #[test]
    fn test_from_env_missing_api_key() {
        std::env::remove_var("JUP_API_KEY");
        let result = Client::from_env();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), JupiterError::Config(_)));
    }

    #[test]
    fn test_from_env_invalid_slippage() {
        std::env::set_var("JUP_API_KEY", "test-key");
        std::env::set_var("JUP_SLIPPAGE_BPS", "not-a-number");
        let result = Client::from_env();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), JupiterError::Config(_)));
        std::env::remove_var("JUP_API_KEY");
        std::env::remove_var("JUP_SLIPPAGE_BPS");
    }

    #[test]
    fn test_debug_redacts_api_key() {
        let client = Client::new("secret-key-123", DEFAULT_API_URL, 100);
        let debug = format!("{client:?}");
        assert!(!debug.contains("secret-key-123"));
        assert!(debug.contains("[redacted]"));
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
    fn test_quote_response_deserialization() {
        let json = serde_json::json!({
            "inputMint": "hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux",
            "inAmount": "100000000",
            "outputMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "outAmount": "450000",
            "otherAmountThreshold": "445500",
            "swapMode": "ExactIn",
            "slippageBps": 100,
            "priceImpactPct": "0.01",
            "routePlan": [{
                "swapInfo": {
                    "ammKey": "abc123",
                    "label": "Orca",
                    "inputMint": "hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux",
                    "outputMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    "inAmount": "100000000",
                    "outAmount": "450000",
                    "feeAmount": "100",
                    "feeMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
                },
                "percent": 100
            }]
        });

        let quote: QuoteResponse = serde_json::from_value(json).unwrap();
        assert_eq!(
            quote.input_mint,
            "hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux"
        );
        assert_eq!(
            quote.output_mint,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(quote.in_amount, "100000000");
        assert_eq!(quote.out_amount, "450000");
        assert_eq!(quote.slippage_bps, 100);
        assert_eq!(quote.route_plan.len(), 1);
        assert_eq!(quote.route_plan[0].swap_info.label.as_deref(), Some("Orca"));
        assert_eq!(quote.route_plan[0].percent, 100);
        // Extra fields are captured
        assert!(quote.extra.get("swapMode").is_some());
        assert!(quote.extra.get("otherAmountThreshold").is_some());
    }
}
