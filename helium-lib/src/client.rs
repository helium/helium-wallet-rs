use crate::{
    anchor_lang::AccountDeserialize,
    asset,
    error::{DecodeError, Error},
    is_zero,
    keypair::{self, Keypair, Pubkey},
    solana_client,
    solana_sdk::{commitment_config::CommitmentConfig, signer::Signer},
};

use futures::{stream, StreamExt, TryStreamExt};
use itertools::Itertools;
use jsonrpc_client::{JsonRpcError, SendRequest};
use std::{marker::Send, sync::Arc};
use tracing::instrument;

pub use solana_client::nonblocking::rpc_client::RpcClient as SolanaRpcClient;

pub static ONBOARDING_URL_MAINNET: &str = "https://onboarding.dewi.org/api/v3";
pub static ONBOARDING_URL_DEVNET: &str = "https://onboarding.web.test-helium.com/api/v3";

pub static VERIFIER_URL_MAINNET: &str = "https://ecc-verifier.web.helium.io";
pub static VERIFIER_URL_DEVNET: &str = "https://ecc-verifier.web.test-helium.com";

pub static SOLANA_URL_MAINNET: &str = "https://solana-rpc.web.helium.io:443?session-key=Pluto";
pub static SOLANA_URL_DEVNET: &str = "https://solana-rpc.web.test-helium.com?session-key=Pluto";
pub static SOLANA_URL_MAINNET_ENV: &str = "SOLANA_MAINNET_URL";
pub static SOLANA_URL_DEVNET_ENV: &str = "SOLANA_DEVNET_URL";

static USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

#[derive(Clone)]
pub struct SolanaClient {
    pub inner: Arc<SolanaRpcClient>,
    pub base_url: String,
    pub wallet: Option<Arc<Keypair>>,
}

impl Default for SolanaClient {
    fn default() -> Self {
        // safe to unwrap
        Self::new(SOLANA_URL_MAINNET, None).unwrap()
    }
}

impl SolanaClient {
    pub fn new(url: &str, wallet: Option<Arc<Keypair>>) -> Result<Self, Error> {
        let client = Arc::new(
            solana_client::nonblocking::rpc_client::RpcClient::new_with_commitment(
                url.to_string(),
                CommitmentConfig::finalized(),
            ),
        );

        Ok(Self {
            inner: client,
            base_url: url.to_string(),
            wallet,
        })
    }

    pub fn ws_url(&self) -> String {
        self.base_url
            .replace("https", "wss")
            .replace("http", "ws")
            .replace("127.0.0.1:8899", "127.0.0.1:8900")
    }

    pub fn pubkey(&self) -> Result<Pubkey, Error> {
        self.wallet
            .as_ref()
            .map(|wallet| wallet.pubkey())
            .ok_or_else(|| Error::WalletUnconfigured)
    }
}

#[derive(Clone)]
pub struct Client {
    pub solana_client: Arc<SolanaClient>,
    pub das_client: Arc<DasClient>,
}

impl TryFrom<&str> for Client {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        fn env_or(key: &str, default: &str) -> String {
            std::env::var(key).unwrap_or_else(|_| default.to_string())
        }
        let url = match value {
            "m" | "mainnet-beta" => &env_or(SOLANA_URL_MAINNET_ENV, SOLANA_URL_MAINNET),
            "d" | "devnet" => &env_or(SOLANA_URL_DEVNET_ENV, SOLANA_URL_DEVNET),
            url => url,
        };
        let das_client = Arc::new(DasClient::with_base_url(url)?);
        let solana_client = Arc::new(SolanaClient::new(url, None)?);

        Ok(Self {
            solana_client,
            das_client,
        })
    }
}

impl AsRef<SolanaRpcClient> for SolanaClient {
    fn as_ref(&self) -> &SolanaRpcClient {
        &self.inner
    }
}

impl AsRef<SolanaRpcClient> for Client {
    fn as_ref(&self) -> &SolanaRpcClient {
        &self.solana_client.inner
    }
}

impl AsRef<DasClient> for Client {
    fn as_ref(&self) -> &DasClient {
        &self.das_client
    }
}

#[async_trait::async_trait]
pub trait GetAnchorAccount {
    async fn anchor_account<T: AccountDeserialize>(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Option<T>, Error>;
    async fn anchor_accounts<T: AccountDeserialize + Send>(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<Option<T>>, Error>;
}

#[async_trait::async_trait]
impl GetAnchorAccount for SolanaRpcClient {
    async fn anchor_account<T: AccountDeserialize>(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Option<T>, Error> {
        let account = self.get_account(pubkey).await?;
        let decoded = T::try_deserialize(&mut account.data.as_ref())?;
        Ok(Some(decoded))
    }

    async fn anchor_accounts<T: AccountDeserialize + Send>(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<Option<T>>, Error> {
        async fn get_accounts<A: AccountDeserialize + Send>(
            client: &solana_client::nonblocking::rpc_client::RpcClient,
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
    ) -> Result<Option<T>, Error> {
        self.solana_client.inner.anchor_account(pubkey).await
    }
    async fn anchor_accounts<T: AccountDeserialize + Send>(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<Option<T>>, Error> {
        self.solana_client.inner.anchor_accounts(pubkeys).await
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

impl DasClientError {
    pub fn is_account_not_found(&self) -> bool {
        match self {
            Self::Rpc(jsonrpc_client::Error::JsonRpc(jsonrpc_client::JsonRpcError {
                message,
                ..
            })) => message.starts_with("Database Error: RecordNotFound"),
            _other => false,
        }
    }
}

#[jsonrpc_client::api]
pub trait DAS {}

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

pub mod config {
    use super::*;
    use crate::{
        dao::SubDao,
        hotspot::{HotspotInfo, HotspotMode, MobileDeviceType},
    };
    use helium_proto::{
        services::{Channel, Endpoint, Uri},
        Message,
    };
    use std::{collections::HashMap, time::Duration};
    use stream::BoxStream;

    pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    pub const RPC_TIMEOUT: Duration = Duration::from_secs(5);
    pub const RPC_TCP_KEEPALIVE: Duration = Duration::from_secs(100);

    trait MessageSign {
        /// Sign the given message
        fn sign<K: AsRef<helium_crypto::Keypair>>(&mut self, keypair: K) -> Result<(), Error>;
    }

    trait MessageVerify {
        fn verify(&self, address: &helium_crypto::PublicKey) -> Result<(), Error>;
    }

    macro_rules! impl_message_sign {
        ($type: ty) => {
            impl MessageSign for $type {
                fn sign<K>(&mut self, keypair: K) -> Result<(), Error>
                where
                    K: AsRef<helium_crypto::Keypair>,
                {
                    use helium_crypto::Sign as _;
                    self.signature = keypair.as_ref().sign(&self.encode_to_vec())?;
                    Ok(())
                }
            }
        };
    }

    macro_rules! impl_message_verify {
        ($type: ty) => {
            impl MessageVerify for $type {
                fn verify(&self, pub_key: &helium_crypto::PublicKey) -> Result<(), Error> {
                    use helium_crypto::Verify as _;
                    let mut _msg = self.clone();
                    _msg.signature = vec![];
                    let buf = _msg.encode_to_vec();
                    pub_key.verify(&buf, &self.signature).map_err(Error::from)
                }
            }
        };
    }

    fn channel_for_uri(uri: &str) -> Result<Channel, DecodeError> {
        let uri: Uri = uri
            .parse()
            .map_err(|_| DecodeError::other(format!("invalid config url: {uri}")))?;
        let channel = Endpoint::from(uri)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(RPC_TIMEOUT)
            .tcp_keepalive(Some(RPC_TCP_KEEPALIVE))
            .connect_lazy();
        Ok(channel)
    }

    #[derive(Clone)]
    pub enum Client {
        Iot(iot::Client),
        Mobile(mobile::Client),
    }

    impl Client {
        pub fn for_subdao(
            subdao: SubDao,
            config: &str,
            address: helium_crypto::PublicKey,
            keypair: Arc<helium_crypto::Keypair>,
        ) -> Result<Self, Error> {
            let result = match subdao {
                SubDao::Iot => Self::Iot(iot::Client::new(config, address, keypair)?),
                SubDao::Mobile => Self::Mobile(mobile::Client::new(config, address, keypair)?),
            };
            Ok(result)
        }

        pub async fn info(
            &mut self,
            address: &helium_crypto::PublicKey,
        ) -> Result<Option<HotspotInfo>, Error> {
            match self {
                Self::Iot(client) => client.info(address).await,
                Self::Mobile(client) => client.info(address).await,
            }
        }

        pub async fn batch_info(
            &mut self,
            addresses: &[helium_crypto::PublicKey],
        ) -> Result<HashMap<helium_crypto::PublicKey, HotspotInfo>, Error> {
            match self {
                Self::Iot(client) => client.batch_info(addresses).await,
                Self::Mobile(client) => client.batch_info(addresses).await,
            }
        }

        pub async fn stream_info(
            &mut self,
        ) -> Result<BoxStream<Result<(helium_crypto::PublicKey, Option<HotspotInfo>), Error>>, Error>
        {
            match self {
                Self::Iot(client) => client.stream_info().await,
                Self::Mobile(client) => client.stream_info().await,
            }
        }
    }

    pub mod iot {
        use super::*;
        use helium_proto::services::iot_config::{
            GatewayClient, GatewayInfo, GatewayInfoReqV1, GatewayInfoResV1, GatewayInfoStreamReqV1,
            GatewayInfoStreamResV1,
        };

        impl_message_sign!(GatewayInfoReqV1);
        impl_message_sign!(GatewayInfoStreamReqV1);
        impl_message_verify!(GatewayInfoResV1);
        impl_message_verify!(GatewayInfoStreamResV1);

        #[derive(Clone)]
        pub struct Client {
            keypair: Arc<helium_crypto::Keypair>,
            client: GatewayClient<Channel>,
            address: helium_crypto::PublicKey,
        }

        impl Client {
            pub fn new(
                uri: &str,
                address: helium_crypto::PublicKey,
                keypair: Arc<helium_crypto::Keypair>,
            ) -> Result<Self, Error> {
                let channel = channel_for_uri(uri)?;
                let client = GatewayClient::new(channel);
                Ok(Self {
                    client,
                    address,
                    keypair,
                })
            }

            pub async fn info(
                &mut self,
                address: &helium_crypto::PublicKey,
            ) -> Result<Option<HotspotInfo>, Error> {
                let mut req = GatewayInfoReqV1 {
                    signer: self.keypair.public_key().into(),
                    address: address.into(),
                    signature: vec![],
                };
                req.sign(&self.keypair)?;
                match self.client.info(req).await {
                    Ok(resp) => {
                        let inner = resp.into_inner();
                        inner.verify(&self.address)?;
                        info_from_res(inner)
                    }
                    Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
                    Err(err) => Err(err.into()),
                }
            }

            pub async fn batch_info(
                &mut self,
                addresses: &[helium_crypto::PublicKey],
            ) -> Result<HashMap<helium_crypto::PublicKey, HotspotInfo>, Error> {
                let map = stream::iter(addresses.to_vec())
                    .map(|address| (address, self.clone()))
                    .map(move |(address, mut client)| async move {
                        let info = client.info(&address).await?;
                        let tuple = info.map(|info| (address, info));
                        Ok::<_, Error>(tuple)
                    })
                    .buffered(10)
                    .try_filter_map(|maybe_tuple| async move { Ok(maybe_tuple) })
                    .try_collect::<Vec<(helium_crypto::PublicKey, HotspotInfo)>>()
                    .await?
                    .into_iter()
                    .collect();
                Ok(map)
            }

            pub async fn stream_info(
                &mut self,
            ) -> Result<
                BoxStream<Result<(helium_crypto::PublicKey, Option<HotspotInfo>), Error>>,
                Error,
            > {
                let mut req = GatewayInfoStreamReqV1 {
                    signer: self.keypair.public_key().into(),
                    batch_size: 1000,
                    signature: vec![],
                };
                req.sign(&self.keypair)?;
                let streaming = self.client.info_stream(req).await?.into_inner();
                let streaming = streaming
                    .map_err(Error::from)
                    .and_then(|res| {
                        let address = self.address.clone();
                        async move {
                            res.verify(&address)?;
                            Ok(res)
                        }
                    })
                    .map_ok(|res| stream::iter(res.gateways).map(info_from_info))
                    .try_flatten();

                Ok(streaming.boxed())
            }
        }

        fn info_from_res(res: GatewayInfoResV1) -> Result<Option<HotspotInfo>, Error> {
            let Some(info) = res.info else {
                return Ok(None);
            };
            info_from_info(info).map(|(_, maybe_info)| maybe_info)
        }

        fn info_from_info(
            info: GatewayInfo,
        ) -> Result<(helium_crypto::PublicKey, Option<HotspotInfo>), Error> {
            let address = info.address.try_into()?;
            let Some(metadata) = info.metadata else {
                return Ok((address, None));
            };

            let mode = if info.is_full_hotspot {
                HotspotMode::Full
            } else {
                HotspotMode::DataOnly
            };
            Ok((
                address,
                Some(HotspotInfo::Iot {
                    mode,
                    gain: Some(rust_decimal::Decimal::new(metadata.gain.into(), 1)),
                    elevation: Some(metadata.elevation),
                    location: metadata.location.parse().ok(),
                    location_asserts: 0,
                }),
            ))
        }
    }

    pub mod mobile {
        use super::*;
        use helium_proto::services::mobile_config::{
            DeviceType, GatewayClient, GatewayInfo, GatewayInfoBatchReqV1, GatewayInfoReqV1,
            GatewayInfoResV1, GatewayInfoStreamReqV1, GatewayInfoStreamResV1,
        };

        impl_message_sign!(GatewayInfoReqV1);
        impl_message_sign!(GatewayInfoStreamReqV1);
        impl_message_sign!(GatewayInfoBatchReqV1);
        impl_message_verify!(GatewayInfoResV1);
        impl_message_verify!(GatewayInfoStreamResV1);

        #[derive(Clone)]
        pub struct Client {
            keypair: Arc<helium_crypto::Keypair>,
            client: GatewayClient<Channel>,
            address: helium_crypto::PublicKey,
        }

        impl Client {
            pub fn new(
                uri: &str,
                address: helium_crypto::PublicKey,
                keypair: Arc<helium_crypto::Keypair>,
            ) -> Result<Self, Error> {
                let channel = channel_for_uri(uri)?;
                let client = GatewayClient::new(channel);
                Ok(Self {
                    client,
                    address,
                    keypair,
                })
            }

            pub async fn info(
                &mut self,
                address: &helium_crypto::PublicKey,
            ) -> Result<Option<HotspotInfo>, Error> {
                let mut req = GatewayInfoReqV1 {
                    signer: self.keypair.public_key().into(),
                    address: address.into(),
                    signature: vec![],
                };
                req.sign(&self.keypair)?;
                match self.client.info(req).await {
                    Ok(resp) => {
                        let inner = resp.into_inner();
                        inner.verify(&self.address)?;
                        info_from_res(inner)
                    }
                    Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
                    Err(err) => Err(err.into()),
                }
            }

            pub async fn batch_info(
                &mut self,
                addresses: &[helium_crypto::PublicKey],
            ) -> Result<HashMap<helium_crypto::PublicKey, HotspotInfo>, Error> {
                let mut req = GatewayInfoBatchReqV1 {
                    batch_size: 1000,
                    addresses: addresses.iter().map(|address| address.to_vec()).collect(),
                    signer: self.keypair.public_key().into(),
                    signature: vec![],
                };
                req.sign(&self.keypair)?;
                let mut result = Vec::with_capacity(addresses.len());
                let mut stream = self.client.info_batch(req).await?.into_inner();
                loop {
                    match stream.try_next().await {
                        Ok(Some(res)) => {
                            res.verify(&self.address)?;
                            let infos: Vec<(helium_crypto::PublicKey, HotspotInfo)> = res
                                .gateways
                                .into_iter()
                                .map(info_from_info)
                                .filter_map_ok(|(address, maybe_info)| {
                                    maybe_info.map(|info| (address, info))
                                })
                                .try_collect()?;
                            result.extend_from_slice(&infos);
                        }
                        Ok(None) => break,
                        Err(err) => return Err(err.into()),
                    }
                }
                Ok(result.into_iter().collect())
            }

            pub async fn stream_info(
                &mut self,
            ) -> Result<
                BoxStream<Result<(helium_crypto::PublicKey, Option<HotspotInfo>), Error>>,
                Error,
            > {
                let mut req = GatewayInfoStreamReqV1 {
                    signer: self.keypair.public_key().into(),
                    batch_size: 1000,
                    ..Default::default()
                };
                req.sign(&self.keypair)?;
                let streaming = self.client.info_stream(req).await?.into_inner();
                let streaming = streaming
                    .map_err(Error::from)
                    .and_then(|res| {
                        let address = self.address.clone();
                        async move {
                            res.verify(&address)?;
                            Ok(res)
                        }
                    })
                    .map_ok(|res| stream::iter(res.gateways).map(info_from_info))
                    .try_flatten();

                Ok(streaming.boxed())
            }
        }

        fn info_from_res(res: GatewayInfoResV1) -> Result<Option<HotspotInfo>, Error> {
            let Some(info) = res.info else {
                return Ok(None);
            };
            info_from_info(info).map(|(_, maybe_info)| maybe_info)
        }

        fn info_from_info(
            info: GatewayInfo,
        ) -> Result<(helium_crypto::PublicKey, Option<HotspotInfo>), Error> {
            let address = info.address.try_into()?;
            let Some(metadata) = info.metadata else {
                return Ok((address, None));
            };

            let (device_type, mode) = match DeviceType::try_from(info.device_type)
                .map_err(DecodeError::from)?
            {
                DeviceType::Cbrs => (MobileDeviceType::Cbrs, HotspotMode::Full),
                DeviceType::WifiIndoor => (MobileDeviceType::WifiIndoor, HotspotMode::Full),
                DeviceType::WifiOutdoor => (MobileDeviceType::WifiOutdoor, HotspotMode::Full),
                DeviceType::WifiDataOnly => (MobileDeviceType::WifiDataOnly, HotspotMode::DataOnly),
            };

            Ok((
                address,
                Some(HotspotInfo::Mobile {
                    device_type,
                    mode,
                    location: metadata.location.parse().ok(),
                    location_asserts: 0,
                    deployment_info: None,
                }),
            ))
        }
    }
}
