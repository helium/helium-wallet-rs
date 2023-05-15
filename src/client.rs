use crate::{
    dao::SubDao,
    hotspot::{Hotspot, HotspotInfo},
    keypair::{Keypair, Pubkey},
    result::{anyhow, Error, Result},
    token::{Token, TokenAmount},
};
use anchor_client::{
    solana_client::rpc_client::RpcClient as SolanaRpcClient, solana_sdk::signer::Signer,
    Client as AnchorClient,
};
use http::Uri;
use jsonrpc::Client as JsonRpcClient;
use rayon::prelude::*;
use reqwest::blocking::Client as RestClient;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;
use std::{boxed::Box, collections::HashMap, rc::Rc, result::Result as StdResult, str::FromStr};

static USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);
static SESSION_KEY_URL: &str = "https://wallet-api-v2.helium.com/api/sessionKey";

pub struct Client {
    settings: Settings,
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
    fn mk_anchor_client(&self, payer: Rc<dyn Signer>) -> Result<AnchorClient> {
        let cluster = anchor_client::Cluster::from_str(&self.to_string())?;
        Ok(AnchorClient::new(cluster, payer.clone()))
    }

    fn mk_solana_client(&self) -> Result<SolanaRpcClient> {
        Ok(SolanaRpcClient::new(self.to_string()))
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

pub type TokenBalances = HashMap<Token, TokenAmount>;

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

    pub fn get_balances(&self, account: &Pubkey) -> Result<TokenBalances> {
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Balances {
            native_balance: u64,
            tokens: Vec<TokenBalance>,
        }

        #[derive(Debug, Deserialize)]
        struct TokenBalance {
            amount: u64,
            mint: String,
        }

        impl TryFrom<Balances> for TokenBalances {
            type Error = Error;
            fn try_from(value: Balances) -> Result<Self> {
                let map: TokenBalances = value
                    .tokens
                    .iter()
                    .filter_map(|entry| {
                        Pubkey::from_str(&entry.mint).ok().and_then(|mint_key| {
                            Token::from_mint(mint_key)
                                .map(|token| (token, token.to_balance(entry.amount)))
                        })
                    })
                    .chain([(Token::Sol, Token::Sol.to_balance(value.native_balance))])
                    .collect();
                Ok(map)
            }
        }

        let url = format!("{}/v0/addresses/{account}/balances", self.settings.url);
        let response = Settings::mk_rest_client()?
            .get(url)
            .query(&[("session-key", &self.settings.session_key)])
            .send()?;
        let balances: Balances = response.json()?;
        let map = balances.try_into()?;
        Ok(map)
    }

    pub fn get_hotspots(&self, owner: &Pubkey) -> Result<Vec<Hotspot>> {
        #[derive(Deserialize)]
        struct PagedResult {
            items: Vec<HotspotResult>,
        }

        impl TryFrom<PagedResult> for Vec<Hotspot> {
            type Error = Error;
            fn try_from(value: PagedResult) -> StdResult<Self, Self::Error> {
                value
                    .items
                    .into_iter()
                    .map(Hotspot::try_from)
                    .collect::<Result<Vec<Hotspot>>>()
            }
        }

        #[derive(Debug, Deserialize)]
        struct HotspotResult {
            content: HotspotContent,
        }

        #[derive(Debug, Deserialize)]
        struct HotspotContent {
            metadata: HotspotMetadata,
        }

        #[derive(Debug, Deserialize)]
        struct HotspotMetadata {
            attributes: Vec<HotspotMetadataAttribute>,
        }

        impl HotspotMetadata {
            fn get_attribute(&self, trait_type: &str) -> Option<&serde_json::Value> {
                self.attributes
                    .iter()
                    .filter(|entry| entry.trait_type == trait_type)
                    .collect::<Vec<&HotspotMetadataAttribute>>()
                    .first()
                    .map(|entry| &entry.value)
            }
        }

        #[derive(Debug, Deserialize)]
        struct HotspotMetadataAttribute {
            value: serde_json::Value,
            trait_type: String,
        }

        impl TryFrom<HotspotResult> for Hotspot {
            type Error = Error;
            fn try_from(value: HotspotResult) -> StdResult<Self, Self::Error> {
                let ecc_key = value
                    .content
                    .metadata
                    .get_attribute("ecc_compact")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("no ecc_compact key found"))
                    .and_then(|str| helium_crypto::PublicKey::from_str(str).map_err(Error::from))?;
                Self::for_address(ecc_key, None)
            }
        }

        let base_params = json!({
            "creatorVerified": true,
            "creatorAddress": "Fv5hf1Fg58htfC7YEXKNEfkpuogUUQDDTLgjGWxxv48H",
            "ownerAddress": owner.to_string(),
        });
        let mut page = 1;
        let mut results = vec![];
        let client = self.settings.mk_jsonrpc_client()?;
        loop {
            let mut params = base_params.clone();
            params["page"] = page.into();
            let page_result: PagedResult = client.call("searchAssets", &[jsonrpc::arg(params)])?;
            println!("PAGE {page}");
            if page_result.items.is_empty() {
                break;
            }
            let hotspots: Vec<Hotspot> = page_result.try_into()?;
            results.extend(hotspots);
            page += 1;
        }

        Ok(results)
    }

    fn get_hotspot_info_in_dao(
        settings: &Settings,
        sub_dao: &SubDao,
        key: &helium_crypto::PublicKey,
    ) -> Result<Option<HotspotInfo>> {
        fn maybe_info<T>(
            result: StdResult<T, anchor_client::ClientError>,
        ) -> crate::result::Result<Option<HotspotInfo>>
        where
            T: Into<HotspotInfo>,
        {
            match result {
                Ok(account) => Ok(Some(account.into())),
                Err(anchor_client::ClientError::AccountNotFound) => Ok(None),
                Err(err) => Err(err.into()),
            }
        }

        let client = settings.mk_anchor_client(Rc::new(Keypair::void()))?;
        let hotspot_key = sub_dao.info_key(key)?;
        let program = client.program(helium_entity_manager::id());
        match sub_dao {
            SubDao::Iot => {
                maybe_info(program.account::<helium_entity_manager::IotHotspotInfoV0>(hotspot_key))
            }
            SubDao::Mobile => maybe_info(
                program.account::<helium_entity_manager::MobileHotspotInfoV0>(hotspot_key),
            ),
        }
    }

    pub fn get_hotspot_info(
        &self,
        sub_daos: &[SubDao],
        key: &helium_crypto::PublicKey,
    ) -> Result<Hotspot> {
        let settings = self.settings.clone();
        let infos = sub_daos
            .par_iter()
            .filter_map(
                |sub_dao| match Self::get_hotspot_info_in_dao(&settings, sub_dao, key) {
                    Ok(Some(metadata)) => Some(Ok((*sub_dao, metadata))),
                    Ok(None) => None,
                    Err(err) => Some(Err(err)),
                },
            )
            .collect::<Result<Vec<(SubDao, HotspotInfo)>>>()?;
        Hotspot::for_address(key.clone(), Some(HashMap::from_iter(infos)))
    }

    pub fn get_pyth_price(&self, token: Token) -> Result<Decimal> {
        let price_key = token
            .price_key()
            .ok_or_else(|| anyhow!("No pyth price key for {token}"))?;
        let client = self.settings.mk_solana_client()?;
        let mut price_account = client.get_account(price_key)?;
        let price_feed =
            pyth_sdk_solana::load_price_feed_from_account(&price_key, &mut price_account)?;

        use std::time::{SystemTime, UNIX_EPOCH};
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let token_price = price_feed
            .get_ema_price_no_older_than(current_time.as_secs().try_into()?, 10 * 60)
            .ok_or_else(|| anyhow!("No token price found"))?;

        // Remove the confidence from the price to use the most conservative price
        // https://docs.pyth.network/pythnet-price-feeds/best-practices
        let price_with_conf = token_price
            .price
            .checked_sub(i64::try_from(token_price.conf.checked_mul(2).unwrap()).unwrap())
            .unwrap();
        Ok(Decimal::new(price_with_conf, token_price.expo.abs() as u32))
    }
}
