use crate::{
    cmd::USER_AGENT,
    keypair::PublicKey,
    result::{anyhow, Result},
    traits::B64,
};
use helium_proto::BlockchainTxn;
use serde_json::json;
use std::time::Duration;

/// The default timeout for API requests
pub const DEFAULT_TIMEOUT: u64 = 120;
/// The default base URL if none is specified.
pub const DEFAULT_BASE_URL: &str = "https://onboarding.dewi.org/api/v2";

pub struct Client {
    base_url: String,
    client: reqwest::Client,
}

impl Default for Client {
    /// Create a new client using the hosted Helium API at
    /// explorer.helium.foundation
    fn default() -> Self {
        Self::new_with_base_url(DEFAULT_BASE_URL.to_string())
    }
}

impl Client {
    /// Create a new client using a given base URL and a default
    /// timeout. The library will use absoluate paths based on this
    /// base_url.
    pub fn new_with_base_url(base_url: String) -> Self {
        Self::new_with_timeout(base_url, DEFAULT_TIMEOUT)
    }

    /// Create a new client using a given base URL, and request
    /// timeout value.  The library will use absoluate paths based on
    /// the given base_url.
    pub fn new_with_timeout(base_url: String, timeout: u64) -> Self {
        let client = reqwest::Client::builder()
            .gzip(true)
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(timeout))
            .build()
            .unwrap();
        Self { base_url, client }
    }

    /// Fetch the public maker key for a given onboarding key
    pub async fn address_for(&self, gateway: &PublicKey) -> Result<PublicKey> {
        let request_url = format!("{}/hotspots/{}", self.base_url, gateway);
        let response: serde_json::Value = self
            .client
            .get(&request_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        response["data"]["maker"]["address"]
            .as_str()
            .map_or(Err(anyhow!("Invalid staking address from server")), |v| {
                v.parse().map_err(|e: helium_crypto::Error| e.into())
            })
    }

    /// Get the staking server to sign a given transaction using the
    /// given onboarding key
    pub async fn sign(&self, onboarding_key: &str, txn: &BlockchainTxn) -> Result<BlockchainTxn> {
        let encoded = txn.to_b64()?;
        let json = json!({ "transaction": encoded });

        let request_url = format!("{}/transactions/pay/{}", self.base_url, onboarding_key);
        let response: serde_json::Value = self
            .client
            .post(&request_url)
            .json(&json)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let txn_data = response["data"]["transaction"]
            .as_str()
            .ok_or_else(|| anyhow!("Unexpected transaction response from staking server"))?;
        BlockchainTxn::from_b64(txn_data)
    }
}
