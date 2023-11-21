use crate::{
    keypair::{serde_pubkey, Pubkey, PublicKey},
    result::{anyhow, Result},
    settings::Settings,
    solana_sdk::{self, signer::Signer},
};
use rayon::prelude::*;
use std::{ops::Deref, result::Result as StdResult, str::FromStr};

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Invalid token type: {0}")]
    InvalidToken(String),
}

lazy_static::lazy_static! {
    static ref HNT_MINT: Pubkey = Pubkey::from_str("hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux").unwrap();
    static ref HNT_PRICE_KEY: Pubkey = Pubkey::from_str("7moA1i5vQUpfDwSpK6Pw9s56ahB7WFGidtbL2ujWrVvm").unwrap();

    static ref MOBILE_MINT: Pubkey = Pubkey::from_str("mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6").unwrap();
    static ref IOT_MINT: Pubkey = Pubkey::from_str("iotEVVZLEywoTn1QdwNPddxPWszn3zFhEot3MfL9fns").unwrap();
    static ref DC_MINT: Pubkey = Pubkey::from_str("dcuc8Amr83Wz27ZkQ2K9NS6r8zRpf1J6cvArEBDZDmm").unwrap();
    static ref SOL_MINT: Pubkey = anchor_spl::token::ID;
}

pub fn transfer<C: Clone + Deref<Target = impl Signer> + PublicKey>(
    settings: &Settings,
    transfers: &[(Pubkey, TokenAmount)],
    keypair: C,
) -> Result<solana_sdk::transaction::Transaction> {
    let client = settings.mk_anchor_client(keypair.clone())?;
    let program = client.program(anchor_spl::token::spl_token::id())?;

    let wallet_public_key = keypair.public_key();
    let mut builder = program.request();

    for (payee, token_amount) in transfers {
        let source_pubkey = token_amount
            .token
            .associated_token_adress(&wallet_public_key);
        let destination_pubkey = token_amount.token.associated_token_adress(payee);
        let ix =
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &wallet_public_key,
                payee,
                token_amount.token.mint(),
                &anchor_spl::token::spl_token::id(),
            );
        builder = builder.instruction(ix);

        let ix = anchor_spl::token::spl_token::instruction::transfer_checked(
            &anchor_spl::token::spl_token::id(),
            &source_pubkey,
            token_amount.token.mint(),
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

pub fn get_balance_for_address(
    settings: &Settings,
    pubkey: &Pubkey,
) -> Result<Option<TokenBalance>> {
    let client = settings.mk_solana_client()?;

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

pub fn get_balance_for_addresses(
    settings: &Settings,
    pubkeys: &[Pubkey],
) -> Result<Vec<TokenBalance>> {
    pubkeys
        .par_iter()
        .filter_map(|pubkey| match get_balance_for_address(settings, pubkey) {
            Ok(Some(balance)) => Some(Ok(balance)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        })
        .collect()
}

pub fn get_pyth_price(settings: &Settings, token: Token) -> Result<pyth_sdk_solana::Price> {
    let price_key = token
        .price_key()
        .ok_or_else(|| anyhow!("No pyth price key for {token}"))?;
    let client = settings.mk_solana_client()?;
    let mut price_account = client.get_account(price_key)?;
    let price_feed = pyth_sdk_solana::load_price_feed_from_account(price_key, &mut price_account)?;

    use std::time::{SystemTime, UNIX_EPOCH};
    let current_time = SystemTime::now().duration_since(UNIX_EPOCH)?;
    price_feed
        .get_ema_price_no_older_than(current_time.as_secs().try_into()?, 10 * 60)
        .ok_or_else(|| anyhow!("No token price found"))
}

#[derive(
    Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Copy, clap::ValueEnum, Hash,
)]
#[serde(rename_all = "lowercase")]
pub enum Token {
    Sol,
    Hnt,
    Mobile,
    Iot,
    Dc,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&str)
    }
}

impl std::str::FromStr for Token {
    type Err = TokenError;
    fn from_str(s: &str) -> StdResult<Self, Self::Err> {
        serde_json::from_str(&format!("\"{s}\""))
            .map_err(|_| TokenError::InvalidToken(s.to_string()))
    }
}

impl Token {
    pub fn from_mint(mint: Pubkey) -> Option<Self> {
        let token = match mint {
            mint if mint == *HNT_MINT => Token::Hnt,
            mint if mint == *IOT_MINT => Token::Iot,
            mint if mint == *DC_MINT => Token::Dc,
            mint if mint == *MOBILE_MINT => Token::Mobile,
            mint if mint == *SOL_MINT => Token::Sol,
            _ => return None,
        };

        Some(token)
    }

    pub fn all() -> Vec<Self> {
        vec![Self::Hnt, Self::Iot, Self::Mobile, Self::Dc, Self::Sol]
    }

    pub(crate) fn transferrable_value_parser(s: &str) -> StdResult<Self, TokenError> {
        let transferrable = [Self::Iot, Self::Mobile, Self::Hnt, Self::Sol];
        let result = Self::from_str(s)?;
        if !transferrable.contains(&result) {
            return Err(TokenError::InvalidToken(s.to_string()));
        }
        Ok(result)
    }

    pub fn associated_token_adress(&self, address: &Pubkey) -> Pubkey {
        match self {
            Self::Sol => *address,
            _ => spl_associated_token_account::get_associated_token_address(address, self.mint()),
        }
    }

    pub fn associated_token_adresses(address: &Pubkey) -> Vec<Pubkey> {
        Self::all()
            .iter()
            .map(|token| token.associated_token_adress(address))
            .collect::<Vec<_>>()
    }

    pub fn mint_circuit_breaker_address(&self) -> Pubkey {
        let (circuit_breaker, _) = Pubkey::find_program_address(
            &[b"mint_windowed_breaker", self.mint().as_ref()],
            &circuit_breaker::id(),
        );
        circuit_breaker
    }
}

#[derive(Debug, serde::Serialize)]
pub struct TokenBalance {
    #[serde(with = "serde_pubkey")]
    pub address: Pubkey,
    pub amount: TokenAmount,
}

#[derive(Debug, serde::Serialize)]
pub struct TokenBalanceMap(std::collections::HashMap<Token, TokenBalance>);

impl From<Vec<TokenBalance>> for TokenBalanceMap {
    fn from(value: Vec<TokenBalance>) -> Self {
        Self(
            value
                .into_iter()
                .map(|balance| (balance.amount.token, balance))
                .collect(),
        )
    }
}

#[derive(Debug)]
pub struct TokenAmount {
    pub token: Token,
    pub amount: u64,
}

impl From<&TokenAmount> for f64 {
    fn from(value: &TokenAmount) -> Self {
        match value.token.decimals() {
            0 => value.amount as f64,
            decimals => value.amount as f64 / 10_usize.pow(decimals.into()) as f64,
        }
    }
}

impl serde::Serialize for TokenAmount {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.token.decimals() == 0 {
            serializer.serialize_u64(self.amount)
        } else {
            serializer.serialize_f64(self.into())
        }
    }
}

impl Default for TokenAmount {
    fn default() -> Self {
        Self {
            token: Token::Dc,
            amount: 0,
        }
    }
}

impl TokenAmount {
    pub fn from_f64(token: Token, amount: f64) -> Self {
        let amount = (amount * 10_usize.pow(token.decimals().into()) as f64) as u64;
        Self { token, amount }
    }

    pub fn from_u64(token: Token, amount: u64) -> Self {
        Self { token, amount }
    }
}

impl Token {
    pub fn decimals(&self) -> u8 {
        match self {
            Self::Hnt => 8,
            Self::Iot | Self::Mobile => 6,
            Self::Dc => 0,
            Self::Sol => 9,
        }
    }

    pub fn mint(&self) -> &Pubkey {
        match self {
            Self::Hnt => &HNT_MINT,
            Self::Mobile => &MOBILE_MINT,
            Self::Iot => &IOT_MINT,
            Self::Dc => &DC_MINT,
            Self::Sol => &SOL_MINT,
        }
    }

    pub fn price_key(&self) -> Option<&Pubkey> {
        match self {
            Self::Hnt => Some(&HNT_PRICE_KEY),
            _ => None,
        }
    }

    pub fn to_balance(self, address: Pubkey, amount: u64) -> TokenBalance {
        TokenBalance {
            address,
            amount: TokenAmount::from_u64(self, amount),
        }
    }
}
