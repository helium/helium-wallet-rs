use crate::{
    keypair::Pubkey,
    result::{anyhow, Error, Result},
    token::{Token, TokenBalance},
};
use anchor_client::{
    solana_client,
    solana_client::rpc_client::RpcClient as SolanaRpcClient,
    solana_sdk::{self, signer::Signer},
    Client as AnchorClient,
};
use http::Uri;
use jsonrpc::Client as JsonRpcClient;
use rayon::prelude::*;
use reqwest::blocking::Client as RestClient;
use serde::{Deserialize, Serialize};
use std::{boxed::Box, collections::HashMap, rc::Rc, result::Result as StdResult, str::FromStr};

static USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);
static SESSION_KEY_URL: &str = "https://wallet-api-v2.helium.com/api/sessionKey";

pub static ONBOARDING_URL_MAINNET: &str = "https://onboarding.dewi.org/api/v3";
pub static ONBOARDING_URL_DEVNET: &str = "https://onboarding.web.test-helium.com/api/v3";

mod dc;
mod hotspot;
mod transfer;

pub use hotspot::HotspotAssertion;

pub struct Client {
    pub settings: Settings,
}

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

impl Settings {
    pub fn mk_anchor_client(&self, payer: Rc<dyn Signer>) -> Result<AnchorClient> {
        let cluster = anchor_client::Cluster::from_str(&self.to_string())?;
        Ok(AnchorClient::new_with_options(
            cluster,
            payer.clone(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        ))
    }

    fn mk_solana_client(&self) -> Result<SolanaRpcClient> {
        Ok(SolanaRpcClient::new_with_commitment(
            self.to_string(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        ))
    }

    fn mk_jsonrpc_client(&self) -> Result<JsonRpcClient> {
        let transport = JsonRpcTransport::new(self.clone())?;
        let client = JsonRpcClient::with_transport(transport);
        Ok(client)
    }

    fn mk_rest_client() -> Result<RestClient> {
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

pub type TokenBalanceMap = HashMap<Token, TokenBalance>;

pub fn to_token_balance_map(balances: Vec<TokenBalance>) -> TokenBalanceMap {
    balances
        .into_iter()
        .map(|balance| (balance.amount.token, balance))
        .collect()
}

impl Client {
    pub fn new(url: &str) -> Result<Self> {
        let session_key = Settings::mk_rest_client()?
            .get(SESSION_KEY_URL)
            .send()?
            .json::<SessionKey>()?
            .session_key;

        let url: Uri = url.parse()?;
        let settings = Settings { url, session_key };

        Ok(Self { settings })
    }

    pub fn get_balance_for_address(&self, pubkey: &Pubkey) -> Result<Option<TokenBalance>> {
        let client = self.settings.mk_solana_client()?;

        match client
            .get_account_with_commitment(pubkey, client.commitment())?
            .value
        {
            Some(account) if account.owner == solana_sdk::system_program::ID => {
                Ok(Some(Token::Sol.to_balance(*pubkey, account.lamports)))
            }
            Some(account) => {
                use anchor_client::anchor_lang::AccountDeserialize;
                let token_account =
                    anchor_spl::token::TokenAccount::try_deserialize(&mut account.data.as_slice())?;
                let token =
                    Token::from_mint(token_account.mint).ok_or_else(|| anyhow!("Invalid mint"))?;
                Ok(Some(token.to_balance(*pubkey, token_account.amount)))
            }
            None => Ok(None),
        }
    }

    pub fn get_balance_for_addresses(&self, pubkeys: &[Pubkey]) -> Result<Vec<TokenBalance>> {
        pubkeys
            .par_iter()
            .filter_map(|pubkey| match self.get_balance_for_address(pubkey) {
                Ok(Some(balance)) => Some(Ok(balance)),
                Ok(None) => None,
                Err(err) => Some(Err(err)),
            })
            .collect()
    }

    pub fn get_pyth_price(&self, token: Token) -> Result<pyth_sdk_solana::Price> {
        let price_key = token
            .price_key()
            .ok_or_else(|| anyhow!("No pyth price key for {token}"))?;
        let client = self.settings.mk_solana_client()?;
        let mut price_account = client.get_account(price_key)?;
        let price_feed =
            pyth_sdk_solana::load_price_feed_from_account(price_key, &mut price_account)?;

        use std::time::{SystemTime, UNIX_EPOCH};
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH)?;
        price_feed
            .get_ema_price_no_older_than(current_time.as_secs().try_into()?, 10 * 60)
            .ok_or_else(|| anyhow!("No token price found"))
    }

    pub fn verify_helium_key(
        &self,
        verifier: &str,
        msg: &[u8],
        signature: &[u8],
        tx: solana_sdk::transaction::Transaction,
    ) -> Result<solana_sdk::transaction::Transaction> {
        #[derive(Deserialize, Serialize, Default)]
        struct VerifyRequest<'a> {
            // hex encoded solana transaction
            pub transaction: &'a str,
            // hex encoded signed message
            pub msg: &'a str,
            // hex encoded signature
            pub signature: &'a str,
        }
        #[derive(Deserialize, Serialize, Default)]
        struct VerifyResponse {
            // hex encoded solana transaction
            pub transaction: String,
        }

        let client = Settings::mk_rest_client()?;
        let serialized_tx = hex::encode(bincode::serialize(&tx)?);
        let response = client
            .post(format!("{}/verify", verifier))
            .json(&VerifyRequest {
                transaction: &serialized_tx,
                msg: &hex::encode(msg),
                signature: &hex::encode(signature),
            })
            .send()?
            .json::<VerifyResponse>()?;
        let signed_tx = bincode::deserialize(&hex::decode(response.transaction)?)?;
        Ok(signed_tx)
    }

    pub fn simulate_transaction(
        &self,
        tx: &solana_sdk::transaction::Transaction,
    ) -> Result<solana_client::rpc_response::RpcSimulateTransactionResult> {
        let client = self.settings.mk_solana_client()?;
        Ok(client.simulate_transaction(tx)?.value)
    }

    pub fn send_and_confirm_transaction(
        &self,
        tx: &solana_sdk::transaction::Transaction,
        skip_preflight: bool,
    ) -> Result<solana_sdk::signature::Signature> {
        let client = self.settings.mk_solana_client()?;
        let config = solana_client::rpc_config::RpcSendTransactionConfig {
            skip_preflight,
            ..Default::default()
        };
        Ok(client.send_transaction_with_config(tx, config)?)
    }
}
