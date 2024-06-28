use crate::{
    anchor_lang::AccountDeserialize,
    asset,
    error::{DecodeError, Error},
    is_zero,
    keypair::{self, Pubkey},
    solana_client,
};
use futures::{stream, StreamExt, TryStreamExt};
use itertools::Itertools;
use jsonrpc_client::{JsonRpcError, SendRequest};
use std::{marker::Send, sync::Arc};
use tracing::instrument;

pub static ONBOARDING_URL_MAINNET: &str = "https://onboarding.dewi.org/api/v3";
pub static ONBOARDING_URL_DEVNET: &str = "https://onboarding.web.test-helium.com/api/v3";

pub static VERIFIER_URL_MAINNET: &str = "https://ecc-verifier.web.helium.io";
pub static VERIFIER_URL_DEVNET: &str = "https://ecc-verifier.web.test-helium.com";

pub static SOLANA_URL_MAINNET: &str = "https://solana-rpc.web.helium.io:443?session-key=Pluto";
pub static SOLANA_URL_DEVNET: &str = "https://solana-rpc.web.test-helium.com";

pub use solana_client::nonblocking::rpc_client::RpcClient as SolanaRpcClient;

#[derive(Clone)]
pub struct Client {
    pub solana_client: Arc<SolanaRpcClient>,
    pub das_client: Arc<DasClient>,
}

#[async_trait::async_trait]
pub trait GetAnchorAccount {
    async fn anchor_account<T: AccountDeserialize>(&self, pubkey: &Pubkey) -> Result<T, Error>;
    async fn anchor_accounts<T: AccountDeserialize + Send>(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<Option<T>>, Error>;
}

#[async_trait::async_trait]
impl GetAnchorAccount for SolanaRpcClient {
    async fn anchor_account<T: AccountDeserialize>(&self, pubkey: &Pubkey) -> Result<T, Error> {
        let account = self.get_account(pubkey).await?;
        let decoded = T::try_deserialize(&mut account.data.as_ref())?;
        Ok(decoded)
    }

    async fn anchor_accounts<T: AccountDeserialize + Send>(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<Option<T>>, Error> {
        async fn get_accounts<A: AccountDeserialize + Send>(
            client: &SolanaRpcClient,
            pubkeys: &[Pubkey],
        ) -> Result<Vec<Option<A>>, Error> {
            let accounts = client.get_multiple_accounts(pubkeys).await?;
            accounts
                .into_iter()
                .map(|maybe_account| {
                    maybe_account
                        .map(|account| A::try_deserialize(&mut account.data.as_ref()))
                        .transpose()
                        .map_err(Error::from)
                })
                .try_collect()
        }

        let accounts = stream::iter(pubkeys.to_vec())
            .chunks(100)
            .map(|key_chunk| async move { get_accounts::<T>(self, &key_chunk).await })
            .buffered(5)
            .try_collect::<Vec<Vec<Option<T>>>>()
            .await?
            .into_iter()
            .flatten()
            .collect_vec();
        Ok(accounts)
    }
}

#[async_trait::async_trait]
impl GetAnchorAccount for Client {
    async fn anchor_account<T: AccountDeserialize>(
        &self,
        pubkey: &keypair::Pubkey,
    ) -> Result<T, Error> {
        self.solana_client.anchor_account(pubkey).await
    }
    async fn anchor_accounts<T: AccountDeserialize + Send>(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<Option<T>>, Error> {
        self.solana_client.anchor_accounts(pubkeys).await
    }
}

impl TryFrom<&str> for Client {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let url = match value {
            "m" | "mainnet-beta" => SOLANA_URL_MAINNET,
            "d" | "devnet" => SOLANA_URL_DEVNET,
            url => url,
        };

        let das_client = Arc::new(DasClient::with_base_url(url)?);
        let solana_client = Arc::new(SolanaRpcClient::new(url.to_string()));
        Ok(Self {
            solana_client,
            das_client,
        })
    }
}

impl AsRef<SolanaRpcClient> for Client {
    fn as_ref(&self) -> &SolanaRpcClient {
        &self.solana_client
    }
}

impl AsRef<DasClient> for Client {
    fn as_ref(&self) -> &DasClient {
        &self.das_client
    }
}

#[derive(
    serde::Serialize, Default, Debug, Clone, std::hash::Hash, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "camelCase")]
pub struct DasSearchAssetsParams {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub creator_verified: bool,
    #[serde(
        with = "keypair::serde_opt_pubkey",
        skip_serializing_if = "Option::is_none"
    )]
    pub creator_address: Option<Pubkey>,
    #[serde(
        with = "keypair::serde_opt_pubkey",
        skip_serializing_if = "Option::is_none"
    )]
    pub owner_address: Option<Pubkey>,
    #[serde(skip_serializing_if = "is_zero")]
    pub page: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub limit: u32,
}

impl DasSearchAssetsParams {
    pub fn for_owner(owner_address: Pubkey, creator_address: Pubkey) -> Self {
        Self {
            owner_address: Some(owner_address),
            creator_address: Some(creator_address),
            creator_verified: true,
            page: 1,
            ..Default::default()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DasClientError {
    #[error("jsonrpc: {0}")]
    Rpc(#[from] jsonrpc_client::Error<reqwest::Error>),
    #[error("json error {0}")]
    Json(#[from] serde_json::Error),
}

impl From<reqwest::Error> for DasClientError {
    fn from(value: reqwest::Error) -> Self {
        jsonrpc_client::Error::from(value).into()
    }
}

impl From<JsonRpcError> for DasClientError {
    fn from(value: JsonRpcError) -> Self {
        Self::from(jsonrpc_client::Error::JsonRpc(value))
    }
}

// impl From<serde_json::Error> for DasClientError

#[jsonrpc_client::api]
pub trait DAS {}

static USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

#[jsonrpc_client::implement(DAS)]
#[derive(Debug, Clone)]
pub struct DasClient {
    inner: reqwest::Client,
    base_url: reqwest::Url,
}

impl Default for DasClient {
    fn default() -> Self {
        // safe to unwrap
        Self::with_base_url(SOLANA_URL_MAINNET).unwrap()
    }
}

impl DasClient {
    pub fn with_base_url(url: &str) -> Result<Self, Error> {
        let client = reqwest::Client::new();
        let base_url = url.parse().map_err(DecodeError::from)?;
        Ok(Self {
            inner: client,
            base_url,
        })
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn get_asset(&self, address: &Pubkey) -> Result<asset::Asset, DasClientError> {
        let body = jsonrpc_client::Request::new_v2("getAsset")
            .with_argument("id".to_string(), address.to_string())?
            .serialize()?;

        let response = Result::from(
            SendRequest::send_request::<asset::Asset>(self, self.base_url.clone(), body)
                .await?
                .payload,
        )?;
        Ok(response)
    }

    #[instrument(skip(self), level = "trace")]
    pub async fn get_asset_proof(
        &self,
        address: &Pubkey,
    ) -> Result<asset::AssetProof, DasClientError> {
        let body = jsonrpc_client::Request::new_v2("getAssetProof")
            .with_argument("id".to_string(), address.to_string())?
            .serialize()?;

        let response = Result::from(
            SendRequest::send_request::<asset::AssetProof>(self, self.base_url.clone(), body)
                .await?
                .payload,
        )?;
        Ok(response)
    }

    #[instrument(skip(self, params), level = "trace")]
    pub async fn search_assets(
        &self,
        params: DasSearchAssetsParams,
    ) -> Result<asset::AssetPage, DasClientError> {
        let params =
            serde_json::to_value(params).map(|value| value.as_object().unwrap().to_owned())?;
        let mut body = jsonrpc_client::Request::new_v2("searchAssets");
        body.params = jsonrpc_client::Params::ByName(params.to_owned());
        let response = Result::from(
            SendRequest::send_request::<asset::AssetPage>(
                self,
                self.base_url.clone(),
                body.serialize()?,
            )
            .await?
            .payload,
        )?;
        Ok(response)
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
