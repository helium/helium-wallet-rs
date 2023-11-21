use crate::result::{anyhow, Error, Result};
use anchor_client::{
    solana_client::rpc_client::RpcClient as SolanaRpcClient,
    solana_sdk::{self, signer::Signer},
    Client as AnchorClient,
};
use http::Uri;
use jsonrpc::Client as JsonRpcClient;
use reqwest::blocking::Client as RestClient;
use serde::Deserialize;
use std::{boxed::Box, ops::Deref, result::Result as StdResult, str::FromStr};

static USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);
static SESSION_KEY_URL: &str = "https://wallet-api-v2.helium.com/api/sessionKey";

pub static ONBOARDING_URL_MAINNET: &str = "https://onboarding.dewi.org/api/v3";
pub static ONBOARDING_URL_DEVNET: &str = "https://onboarding.web.test-helium.com/api/v3";

pub static VERIFIER_URL_MAINNET: &str = "https://ecc-verifier.web.helium.io";
pub static VERIFIER_URL_DEVNET: &str = "https://ecc-verifier.web.test-helium.com";

pub static SOLANA_URL_MAINNET: &str = "https://solana-rpc.web.helium.io:443";
pub static SOLANA_URL_DEVNET: &str = "https://solana-rpc.web.test-helium.com";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionKey {
    session_key: String,
}

#[derive(Debug, Clone)]
pub struct Settings {
    url: http::Uri,
    session_key: String,
}

impl TryFrom<&Settings> for http::Uri {
    type Error = Error;
    fn try_from(value: &Settings) -> Result<Self> {
        Ok(value.to_string().parse::<Self>()?)
    }
}

impl ToString for Settings {
    fn to_string(&self) -> String {
        format!("{}?session-key={}", self.url, self.session_key)
    }
}

impl TryFrom<&str> for Settings {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self> {
        let session_key = Settings::mk_rest_client()?
            .get(SESSION_KEY_URL)
            .send()?
            .json::<SessionKey>()?
            .session_key;

        let url = match value {
            "m" | "mainnet-beta" => SOLANA_URL_MAINNET,
            "d" | "devnet" => SOLANA_URL_DEVNET,
            url => url,
        };

        let url: Uri = url.parse()?;
        Ok(Self { url, session_key })
    }
}

impl Settings {
    pub fn mk_anchor_client<C: Clone + Deref<Target = impl Signer>>(
        &self,
        payer: C,
    ) -> Result<AnchorClient<C>> {
        let cluster = anchor_client::Cluster::from_str(&self.to_string())?;
        Ok(AnchorClient::new_with_options(
            cluster,
            payer,
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        ))
    }

    pub fn mk_solana_client(&self) -> Result<SolanaRpcClient> {
        Ok(SolanaRpcClient::new_with_commitment(
            self.to_string(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        ))
    }

    pub fn mk_jsonrpc_client(&self) -> Result<JsonRpcClient> {
        let transport = JsonRpcTransport::new(self.clone())?;
        let client = JsonRpcClient::with_transport(transport);
        Ok(client)
    }

    pub fn mk_rest_client() -> Result<RestClient> {
        Ok(RestClient::builder().user_agent(USER_AGENT).build()?)
    }
}

struct JsonRpcTransport {
    client: RestClient,
    settings: Settings,
}

impl JsonRpcTransport {
    fn new(settings: Settings) -> Result<Self> {
        let client = RestClient::builder().user_agent(USER_AGENT).build()?;

        Ok(Self { client, settings })
    }

    fn map_err(err: impl std::error::Error + Send + Sync + 'static) -> jsonrpc::Error {
        jsonrpc::Error::Transport(Box::new(err))
    }

    fn request<R>(&self, req: impl serde::Serialize) -> StdResult<R, jsonrpc::Error>
    where
        R: for<'a> serde::de::Deserialize<'a>,
    {
        let resp = self
            .client
            .post(self.settings.to_string())
            .query(&[("session-key", &self.settings.session_key)])
            .json(&req)
            .send()
            .map_err(Self::map_err)?;
        let bytes = resp.bytes().map_err(Self::map_err)?;
        serde_json::from_slice(&bytes).map_err(|err| err.into())
    }

    fn map_request(req: impl serde::Serialize) -> StdResult<serde_json::Value, jsonrpc::Error> {
        let mut value = serde_json::to_value(req)?;
        let params = value["params"]
            .as_array()
            .and_then(|array| array.first())
            .ok_or_else(|| jsonrpc::Error::Transport(anyhow!("No parameters found").into()))?;
        value["params"] = params.to_owned();
        Ok(value)
    }
}

impl jsonrpc::Transport for JsonRpcTransport {
    fn fmt_target(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}?session-key={}",
            self.settings.url, self.settings.session_key
        )
    }

    fn send_request(&self, req: jsonrpc::Request) -> StdResult<jsonrpc::Response, jsonrpc::Error> {
        let req = Self::map_request(req)?;
        self.request(req)
    }

    fn send_batch(
        &self,
        reqs: &[jsonrpc::Request],
    ) -> StdResult<Vec<jsonrpc::Response>, jsonrpc::Error> {
        let reqs = reqs
            .iter()
            .map(Self::map_request)
            .collect::<StdResult<Vec<serde_json::Value>, jsonrpc::Error>>()?;
        self.request(reqs)
    }
}
