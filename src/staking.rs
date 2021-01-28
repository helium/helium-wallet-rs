use crate::{
    keypair::PubKeyBin,
    result::Result,
    traits::{B58, B64},
};
use helium_api::BlockchainTxn;
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
            .timeout(Duration::from_secs(timeout))
            .build()
            .unwrap();
        Self { base_url, client }
    }

    /// Fetch the public maker key for a given gateway key
    pub fn address_for(&self, gateway: &PubKeyBin) -> Result<PubKeyBin> {
        let request_url = format!("{}/hotspots/{}", self.base_url, gateway.to_b58()?);
        let response: serde_json::Value = self
            .client
            .get(&request_url)
            .send()?
            .error_for_status()?
            .json()?;
        response["data"]["publicAddress"]
            .as_str()
            .map_or(Err("Invalid staking address from server".into()), |v| {
                PubKeyBin::from_b58(&v)
            })
    }

    /// Get the staking server to sign a given transaction using the
    /// given onboarding key
    pub fn sign(&self, onboarding_key: &str, txn: &BlockchainTxn) -> Result<BlockchainTxn> {
        let encoded = txn.to_b64()?;
        let json = json!({ "transaction": encoded });

        let request_url = format!("{}/transactions/pay/{}", self.base_url, onboarding_key);
        let response: serde_json::Value = self
            .client
            .post(&request_url)
            .json(&json)
            .send()?
            .error_for_status()?
            .json()?;
        let txn_data = response["data"]["transaction"]
            .as_str()
            .ok_or("Unexpected transaction response from staking server")?;
        Ok(BlockchainTxn::from_b64(txn_data)?)
    }
}
