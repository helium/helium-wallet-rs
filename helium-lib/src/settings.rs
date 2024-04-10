use crate::{
    asset, keypair,
    result::{DecodeError, Error, Result as CrateResult},
};
use anchor_client::{
    solana_client::nonblocking::rpc_client::RpcClient as SolanaRpcClient, Client as AnchorClient,
};
use jsonrpc_client::SendRequest;
use reqwest::blocking::Client as RestClient;
use serde::Deserialize;
use solana_sdk::signer::Signer;
use std::{ops::Deref, str::FromStr};
use tracing::instrument;

static USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);
static _SESSION_KEY_URL: &str = "https://wallet-api-v2.helium.com/api/sessionKey";

pub static ONBOARDING_URL_MAINNET: &str = "https://onboarding.dewi.org/api/v3";
pub static ONBOARDING_URL_DEVNET: &str = "https://onboarding.web.test-helium.com/api/v3";

pub static VERIFIER_URL_MAINNET: &str = "https://ecc-verifier.web.helium.io";
pub static VERIFIER_URL_DEVNET: &str = "https://ecc-verifier.web.test-helium.com";

pub static SOLANA_URL_MAINNET: &str = "https://solana-rpc.web.helium.io:443?session-key=Pluto";
pub static SOLANA_URL_DEVNET: &str = "https://solana-rpc.web.test-helium.com";

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    url: url::Url,
}

impl TryFrom<&Settings> for url::Url {
    type Error = Error;
    fn try_from(value: &Settings) -> CrateResult<Self> {
        Ok(value
            .to_string()
            .parse::<Self>()
            .map_err(DecodeError::from)?)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            url: SOLANA_URL_MAINNET.parse().unwrap(),
        }
    }
}

impl std::fmt::Display for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.url.fmt(f)
    }
}

impl TryFrom<&str> for Settings {
    type Error = Error;
    fn try_from(value: &str) -> CrateResult<Self> {
        let url = match value {
            "m" | "mainnet-beta" => SOLANA_URL_MAINNET,
            "d" | "devnet" => SOLANA_URL_DEVNET,
            url => url,
        };

        let url: url::Url = url.parse().map_err(DecodeError::from)?;
        Ok(Self { url })
    }
}

impl Settings {
    pub fn mk_anchor_client<C: Clone + Deref<Target = impl Signer>>(
        &self,
        payer: C,
    ) -> CrateResult<AnchorClient<C>> {
        let url_str = self.to_string();
        let cluster = anchor_client::Cluster::from_str(&url_str).map_err(DecodeError::other)?;
        Ok(AnchorClient::new_with_options(
            cluster,
            payer,
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        ))
    }

    pub fn mk_solana_client(&self) -> CrateResult<SolanaRpcClient> {
        Ok(SolanaRpcClient::new_with_commitment(
            self.to_string(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        ))
    }

    pub fn mk_jsonrpc_client(&self) -> CrateResult<DasClient> {
        let client = DasClient::from_settings(self)?;
        Ok(client)
    }

    pub fn mk_rest_client() -> CrateResult<RestClient> {
        Ok(RestClient::builder().user_agent(USER_AGENT).build()?)
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DasSearchAssetsParams {
    pub creator_verified: bool,
    #[serde(with = "keypair::serde_pubkey")]
    pub creator_address: keypair::Pubkey,
    #[serde(with = "keypair::serde_pubkey")]
    pub owner_address: keypair::Pubkey,
    pub page: u32,
}

pub type DasClientError = jsonrpc_client::Error<reqwest::Error>;

#[jsonrpc_client::api]
pub trait DAS {}

#[jsonrpc_client::implement(DAS)]
pub struct DasClient {
    inner: reqwest::Client,
    base_url: reqwest::Url,
}

impl DasClient {
    pub fn from_settings(settings: &Settings) -> CrateResult<Self> {
        let client = reqwest::Client::new();
        let base_url = settings.to_string().parse().map_err(DecodeError::from)?;
        Ok(Self {
            inner: client,
            base_url,
        })
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn get_asset(
        &self,
        address: &keypair::Pubkey,
    ) -> Result<asset::Asset, jsonrpc_client::Error<reqwest::Error>> {
        let body = jsonrpc_client::Request::new_v2("getAsset")
            .with_argument("id".to_string(), address.to_string())?
            .serialize()?;

        let response = Result::from(
            self.inner
                .send_request::<asset::Asset>(self.base_url.clone(), body)
                .await?
                .payload,
        )?;
        Ok(response)
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn get_asset_proof(
        &self,
        address: &keypair::Pubkey,
    ) -> Result<asset::AssetProof, jsonrpc_client::Error<reqwest::Error>> {
        let body = jsonrpc_client::Request::new_v2("getAssetProof")
            .with_argument("id".to_string(), address.to_string())?
            .serialize()?;

        let response = Result::from(
            self.inner
                .send_request::<asset::AssetProof>(self.base_url.clone(), body)
                .await?
                .payload,
        )?;
        Ok(response)
    }

    #[instrument(skip(self, params), level = "trace")]
    pub async fn search_assets(
        &self,
        params: &DasSearchAssetsParams,
    ) -> Result<Vec<asset::Asset>, jsonrpc_client::Error<reqwest::Error>> {
        let body = jsonrpc_client::Request::new_v2("searchAssets")
            .with_argument("creatorVerified".to_string(), params.creator_verified)?
            .with_argument(
                "creatorAddress".to_string(),
                params.creator_address.to_string(),
            )?
            .with_argument("ownerAddress".to_string(), params.owner_address.to_string())?
            .with_argument("page".to_string(), params.page)?
            .serialize()?;

        let response = Result::from(
            self.inner
                .send_request::<asset::AssetPage>(self.base_url.clone(), body)
                .await?
                .payload,
        )?;
        Ok(response.items)
    }
}

#[async_trait::async_trait]
impl jsonrpc_client::SendRequest for DasClient {
    type Error = reqwest::Error;
    async fn send_request<P>(
        &self,
        endpoint: jsonrpc_client::Url,
        body: String,
    ) -> Result<jsonrpc_client::Response<P>, Self::Error>
    where
        P: serde::de::DeserializeOwned,
    {
        self.inner
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .body(body)
            .send()
            .await?
            .json()
            .await
    }
}
