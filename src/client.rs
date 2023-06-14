use crate::{
    dao::{Dao, SubDao},
    hotspot::{Hotspot, HotspotInfo},
    keypair::{Keypair, Pubkey},
    result::{anyhow, Error, Result},
    token::{Token, TokenAmount, TokenBalance},
};
use anchor_client::{
    solana_client,
    solana_client::rpc_client::RpcClient as SolanaRpcClient,
    solana_sdk::{self, signer::Signer},
    Client as AnchorClient,
};
use anchor_spl::associated_token::get_associated_token_address;
use http::Uri;
use jsonrpc::Client as JsonRpcClient;
use rayon::prelude::*;
use reqwest::blocking::Client as RestClient;
use serde::Deserialize;
use serde_json::json;
use std::{boxed::Box, collections::HashMap, rc::Rc, result::Result as StdResult, str::FromStr};

static USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);
static SESSION_KEY_URL: &str = "https://wallet-api-v2.helium.com/api/sessionKey";

pub static ONBOARDING_URL_MAINNET: &str = "https://onboarding.dewi.org/api/v3";
pub static ONBOARDING_URL_DEVNET: &str = "https://onboarding.web.test-helium.com/api/v3";

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
pub struct HotspotAssertion {
    pub location: Option<u64>,
    pub gain: Option<i32>,
    pub elevation: Option<i32>,
}

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
        subdao: &SubDao,
        key: &helium_crypto::PublicKey,
    ) -> Result<Option<HotspotInfo>> {
        fn maybe_info<T>(
            result: StdResult<T, anchor_client::ClientError>,
        ) -> Result<Option<HotspotInfo>>
        where
            T: Into<HotspotInfo>,
        {
            match result {
                Ok(account) => Ok(Some(account.into())),
                Err(anchor_client::ClientError::AccountNotFound) => Ok(None),
                Err(err) => Err(err.into()),
            }
        }

        let client = settings.mk_anchor_client(Keypair::void())?;
        let hotspot_key = subdao.info_key_for_helium_key(key)?;
        let program = client.program(helium_entity_manager::id());
        match subdao {
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
        subdaos: &[SubDao],
        key: &helium_crypto::PublicKey,
    ) -> Result<Hotspot> {
        let settings = self.settings.clone();
        let infos = subdaos
            .par_iter()
            .filter_map(
                |subdao| match Self::get_hotspot_info_in_dao(&settings, subdao, key) {
                    Ok(Some(metadata)) => Some(Ok((*subdao, metadata))),
                    Ok(None) => None,
                    Err(err) => Some(Err(err)),
                },
            )
            .collect::<Result<Vec<(SubDao, HotspotInfo)>>>()?;
        Hotspot::for_address(key.clone(), Some(HashMap::from_iter(infos)))
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

    pub fn mint_dc(
        &self,
        amount: TokenAmount,
        payee: &Pubkey,
        keypair: Rc<Keypair>,
    ) -> Result<solana_sdk::transaction::Transaction> {
        impl TryFrom<TokenAmount> for data_credits::MintDataCreditsArgsV0 {
            type Error = Error;
            fn try_from(value: TokenAmount) -> StdResult<Self, Self::Error> {
                match value.token {
                    Token::Hnt => Ok(Self {
                        hnt_amount: Some(value.amount),
                        dc_amount: None,
                    }),
                    Token::Dc => Ok(Self {
                        hnt_amount: None,
                        dc_amount: Some(value.amount),
                    }),
                    other => Err(anyhow!("Invalid token type: {other}")),
                }
            }
        }

        let client = self.settings.mk_anchor_client(keypair.clone())?;
        let dc_program = client.program(data_credits::id());
        let data_credits = SubDao::dc_key();
        let hnt_price_oracle = dc_program
            .account::<data_credits::DataCreditsV0>(data_credits)?
            .hnt_price_oracle;

        let (circuit_breaker, _) = Pubkey::find_program_address(
            &[b"mint_windowed_breaker", Token::Dc.mint().as_ref()],
            &circuit_breaker::id(),
        );

        let burner = get_associated_token_address(&keypair.pubkey(), Token::Hnt.mint());

        let recipient_token_account = get_associated_token_address(payee, Token::Dc.mint());

        let accounts = data_credits::accounts::MintDataCreditsV0 {
            data_credits,
            owner: keypair.public_key(),
            hnt_mint: *Token::Hnt.mint(),
            dc_mint: *Token::Dc.mint(),
            recipient: *payee,
            recipient_token_account,
            system_program: solana_sdk::system_program::ID,
            token_program: anchor_spl::token::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            hnt_price_oracle,
            circuit_breaker_program: circuit_breaker::id(),
            circuit_breaker,
            burner,
        };

        let args = data_credits::instruction::MintDataCreditsV0 {
            args: amount.try_into()?,
        };
        let tx = dc_program
            .request()
            .accounts(accounts)
            .args(args)
            .signed_transaction()?;
        Ok(tx)
    }

    pub fn delegate_dc(
        &self,
        subdao: SubDao,
        router_key: &str,
        amount: u64,
        keypair: Rc<Keypair>,
    ) -> Result<solana_sdk::transaction::Transaction> {
        let client = self.settings.mk_anchor_client(keypair.clone())?;
        let dc_program = client.program(data_credits::id());

        let delegated_data_credits = subdao.delegated_dc_key(router_key);

        let accounts = data_credits::accounts::DelegateDataCreditsV0 {
            delegated_data_credits,
            data_credits: SubDao::dc_key(),
            dc_mint: *Token::Dc.mint(),
            dao: Dao::Hnt.key(),
            sub_dao: subdao.key(),
            owner: keypair.public_key(),
            from_account: get_associated_token_address(&keypair.public_key(), Token::Dc.mint()),
            escrow_account: subdao.escrow_account_key(&delegated_data_credits),
            payer: keypair.public_key(),
            associated_token_program: anchor_spl::associated_token::ID,
            token_program: anchor_spl::token::ID,
            system_program: solana_sdk::system_program::ID,
        };

        let args = data_credits::instruction::DelegateDataCreditsV0 {
            args: data_credits::DelegateDataCreditsArgsV0 {
                amount,
                router_key: router_key.to_string(),
            },
        };
        let tx = dc_program
            .request()
            .accounts(accounts)
            .args(args)
            .signed_transaction()?;
        Ok(tx)
    }

    pub fn transfer(
        &self,
        transfers: &[(Pubkey, TokenAmount)],
        keypair: Rc<Keypair>,
    ) -> Result<solana_sdk::transaction::Transaction> {
        let client = self.settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(anchor_spl::token::spl_token::id());

        let wallet_public_key = keypair.public_key();
        let mut builder = program.request();

        for (payee, token_amount) in transfers {
            let mint_pubkey = token_amount.token.mint();
            let source_pubkey = get_associated_token_address(&wallet_public_key, mint_pubkey);
            let destination_pubkey = get_associated_token_address(payee, mint_pubkey);
            let ix =
                spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                    &wallet_public_key,
                    payee,
                    mint_pubkey,
                    &anchor_spl::token::spl_token::id(),
                );
            builder = builder.instruction(ix);

            let ix = anchor_spl::token::spl_token::instruction::transfer_checked(
                &anchor_spl::token::spl_token::id(),
                &source_pubkey,
                mint_pubkey,
                &destination_pubkey,
                &wallet_public_key,
                &[],
                token_amount.amount,
                token_amount.token.decimals(),
            )?;
            builder = builder.instruction(ix);
        }

        let tx = builder.signed_transaction()?;
        Ok(tx)
    }

    pub fn hotspot_assert(
        &self,
        onboarding_server: &str,
        subdao: SubDao,
        hotspot: &helium_crypto::PublicKey,
        assertion: HotspotAssertion,
        keypair: Rc<Keypair>,
    ) -> Result<solana_sdk::transaction::Transaction> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct OnboardingResponse {
            code: u32,
            success: bool,
            error_message: Option<String>,
            data: Option<OnboardingResponseData>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct OnboardingResponseData {
            solana_transactions: Vec<OnboardingResponseSolanaTransaction>,
        }
        #[derive(Deserialize)]
        struct OnboardingResponseSolanaTransaction {
            data: Vec<u8>,
        }

        let client = Settings::mk_rest_client()?;
        let url = format!(
            "{}/transactions/{}/update-metadata",
            onboarding_server,
            subdao.to_string()
        );
        let mut params = json!({
            "entityKey": hotspot.to_string(),
            "wallet": keypair.public_key().to_string(),
        });

        if let Some(location) = assertion.location {
            params["location"] = location.into();
        }
        if let Some(gain) = assertion.gain {
            params["gain"] = gain.into();
        }
        if let Some(elevation) = assertion.elevation {
            params["elevation"] = elevation.into();
        }

        let resp = client.post(url).json(&params).send()?.error_for_status()?;
        let onboarding_resp = resp.json::<OnboardingResponse>()?;
        if !onboarding_resp.success {
            return Err(anyhow!(
                "Onboard transaction request failed: {} {}",
                onboarding_resp.code,
                onboarding_resp
                    .error_message
                    .unwrap_or_else(|| "unknown".to_string())
            ));
        }

        let mut tx = onboarding_resp
            .data
            .ok_or_else(|| anyhow!("No transaction data returned"))
            .and_then(|resp_data| {
                bincode::deserialize::<solana_sdk::transaction::Transaction>(
                    &resp_data.solana_transactions[0].data,
                )
                .map_err(anyhow::Error::from)
            })?;

        tx.try_partial_sign(&[&*keypair], tx.message.recent_blockhash)?;
        Ok(tx)
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
