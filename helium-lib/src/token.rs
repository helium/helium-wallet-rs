use crate::{
    anchor_lang::AccountDeserialize,
    anchor_spl,
    client::SolanaRpcClient,
    error::{DecodeError, Error},
    keypair::{serde_pubkey, Keypair, Pubkey},
    mk_transaction_with_blockhash,
    solana_sdk::{commitment_config::CommitmentConfig, signer::Signer, system_instruction},
    TransactionWithBlockhash,
};
use chrono::{DateTime, Duration, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use helium_anchor_gen::circuit_breaker;
use std::{collections::HashMap, result::Result as StdResult, str::FromStr};

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Invalid token type: {0}")]
    InvalidToken(String),
}

lazy_static::lazy_static! {
    static ref HNT_MINT: Pubkey = Pubkey::from_str("hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux").unwrap();
    static ref HNT_PRICE_KEY: Pubkey = Pubkey::from_str("4DdmDswskDxXGpwHrXUfn2CNUm9rt21ac79GHNTN3J33").unwrap();
    static ref HNT_PRICE_FEED: price::FeedId = price::feed_from_hex("649fdd7ec08e8e2a20f425729854e90293dcbe2376abc47197a14da6ff339756").unwrap();

    static ref MOBILE_MINT: Pubkey = Pubkey::from_str("mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6").unwrap();
    static ref MOBILE_PRICE_KEY: Pubkey = Pubkey::from_str("DQ4C1tzvu28cwo1roN1Wm6TW35sfJEjLh517k3ZeWevx").unwrap();
    static ref MOBILE_PRICE_FEED: price::FeedId = price::feed_from_hex("ff4c53361e36a9b837433c87d290c229e1f01aec5ef98d9f3f70953a20a629ce").unwrap();

    static ref IOT_MINT: Pubkey = Pubkey::from_str("iotEVVZLEywoTn1QdwNPddxPWszn3zFhEot3MfL9fns").unwrap();
    static ref IOT_PRICE_KEY: Pubkey = Pubkey::from_str("8UYEn5Weq7toHwgcmctvcAxaNJo3SJxXEayM57rpoXr9").unwrap();
    static ref IOT_PRICE_FEED: price::FeedId = price::feed_from_hex("6b701e292e0836d18a5904a08fe94534f9ab5c3d4ff37dc02c74dd0f4901944d").unwrap();

    static ref DC_MINT: Pubkey = Pubkey::from_str("dcuc8Amr83Wz27ZkQ2K9NS6r8zRpf1J6cvArEBDZDmm").unwrap();
    static ref SOL_MINT: Pubkey = solana_sdk::system_program::ID;
}

pub async fn burn<C: AsRef<SolanaRpcClient>>(
    client: &C,
    token_amount: &TokenAmount,
    keypair: &Keypair,
) -> Result<TransactionWithBlockhash, Error> {
    let wallet_pubkey = keypair.pubkey();
    let ix = match token_amount.token.mint() {
        spl_mint if spl_mint == Token::Sol.mint() => {
            return Err(DecodeError::other("native token burn not supported").into());
        }
        spl_mint => {
            let token_account = token_amount.token.associated_token_adress(&wallet_pubkey);
            anchor_spl::token::spl_token::instruction::burn_checked(
                &anchor_spl::token::spl_token::id(),
                &token_account,
                spl_mint,
                &wallet_pubkey,
                &[&wallet_pubkey],
                token_amount.amount,
                token_amount.token.decimals(),
            )?
        }
    };

    let mut txn = mk_transaction_with_blockhash(client, &[ix], &wallet_pubkey).await?;
    txn.try_sign(&[keypair])?;
    Ok(txn)
}

pub async fn transfer<C: AsRef<SolanaRpcClient>>(
    client: &C,
    transfers: &[(Pubkey, TokenAmount)],
    keypair: &Keypair,
) -> Result<TransactionWithBlockhash, Error> {
    let wallet_public_key = keypair.pubkey();

    let mut ixs = vec![];
    for (payee, token_amount) in transfers {
        match token_amount.token.mint() {
            spl_mint if spl_mint == Token::Sol.mint() => {
                let ix =
                    system_instruction::transfer(&wallet_public_key, payee, token_amount.amount);
                ixs.push(ix);
            }
            spl_mint => {
                let source_pubkey = token_amount
                    .token
                    .associated_token_adress(&wallet_public_key);
                let destination_pubkey = token_amount.token.associated_token_adress(payee);
                let ix = spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                    &wallet_public_key,
                    payee,
                    spl_mint,
                    &anchor_spl::token::spl_token::id(),
                );
                ixs.push(ix);

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
                ixs.push(ix);
            }
        }
    }

    let mut txn = mk_transaction_with_blockhash(client, &ixs, &wallet_public_key).await?;
    txn.try_sign(&[keypair])?;
    Ok(txn)
}

pub async fn balance_for_address<C: AsRef<SolanaRpcClient>>(
    client: &C,
    pubkey: &Pubkey,
) -> Result<Option<TokenBalance>, Error> {
    match client
        .as_ref()
        .get_account_with_commitment(pubkey, CommitmentConfig::confirmed())
        .await?
        .value
    {
        Some(account) if account.owner == solana_sdk::system_program::ID => {
            Ok(Some(Token::Sol.to_balance(*pubkey, account.lamports)))
        }
        Some(account) => {
            let token_account =
                anchor_spl::token::TokenAccount::try_deserialize(&mut account.data.as_slice())?;
            let token = Token::from_mint(token_account.mint)
                .ok_or_else(|| DecodeError::other("Invalid mint"))?;
            Ok(Some(token.to_balance(*pubkey, token_account.amount)))
        }
        None => Ok(None),
    }
}

pub async fn balance_for_addresses<C: AsRef<SolanaRpcClient>>(
    client: &C,
    pubkeys: &[Pubkey],
) -> Result<Vec<TokenBalance>, Error> {
    stream::iter(pubkeys)
        .map(|pubkey| balance_for_address(client, pubkey))
        .buffered(10)
        .filter_map(|result| async { result.transpose() })
        .try_collect()
        .await
}

pub mod price {
    use super::*;
    use pyth_solana_receiver_sdk::price_update::{self, PriceUpdateV2};
    use rust_decimal::prelude::*;

    pub use pyth_solana_receiver_sdk::price_update::FeedId;
    pub const DC_PER_USD: i64 = 100_000;

    #[derive(Debug, thiserror::Error)]
    pub enum PriceError {
        #[error("invalid or unsupported token: {0}")]
        InvalidToken(super::Token),
        #[error("invalid price feed")]
        InvalidFeed,
        #[error("price too old")]
        TooOld,
        #[error("price below 0")]
        Negative,
        #[error("invalid price timestamp: {0}")]
        InvalidTimestamp(i64),
        #[error("unsupported positive price exponent")]
        PositiveExponent,
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct Price {
        pub timestamp: DateTime<Utc>,
        pub price: Decimal,
        pub token: super::Token,
    }

    pub fn feed_from_hex(str: &str) -> Result<FeedId, PriceError> {
        let feed_id =
            price_update::get_feed_id_from_hex(str).map_err(|_| PriceError::InvalidFeed)?;
        Ok(feed_id)
    }

    pub async fn get_with_max_age<C: AsRef<SolanaRpcClient>>(
        client: &C,
        token: Token,
        max_age: Duration,
    ) -> Result<Price, Error> {
        use helium_anchor_gen::anchor_lang::AccountDeserialize;
        let price_key = token.price_key().ok_or(PriceError::InvalidToken(token))?;
        let price_feed = token.price_feed().ok_or(PriceError::InvalidToken(token))?;
        let account = client.as_ref().get_account(price_key).await?;
        let PriceUpdateV2 { price_message, .. } =
            PriceUpdateV2::try_deserialize(&mut account.data.as_slice())?;

        if (price_message
            .publish_time
            .saturating_add(max_age.num_seconds()))
            < Utc::now().timestamp()
        {
            return Err(PriceError::TooOld.into());
        }
        if price_message.ema_price < 0 {
            return Err(PriceError::Negative.into());
        }
        if price_message.exponent > 0 {
            return Err(PriceError::PositiveExponent.into());
        }
        if price_message.feed_id != *price_feed {
            return Err(PriceError::InvalidFeed.into());
        }
        let scale = price_message.exponent.unsigned_abs();
        // Remove the confidence interval from the price to get the most optimistic price:
        let mut price = Decimal::new(price_message.ema_price, scale)
            + Decimal::new(price_message.ema_conf as i64, scale) * Decimal::new(2, 0);
        // ensure we use only up to 6 decimals, this rounds using `MidpointAwayFromZero`
        price.rescale(6);
        let timestamp = DateTime::from_timestamp(price_message.publish_time, 0)
            .ok_or(PriceError::InvalidTimestamp(price_message.publish_time))?;

        Ok(Price {
            timestamp,
            price,
            token,
        })
    }

    pub async fn get<C: AsRef<SolanaRpcClient>>(client: &C, token: Token) -> Result<Price, Error> {
        get_with_max_age(client, token, Duration::minutes(10)).await
    }
}

#[derive(
    Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Copy, Hash, PartialOrd, Ord,
)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
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
        let str = match self {
            Token::Sol => "sol",
            Token::Hnt => "hnt",
            Token::Mobile => "mobile",
            Token::Iot => "iot",
            Token::Dc => "dc",
        };
        f.write_str(str)
    }
}

impl FromStr for Token {
    type Err = TokenError;
    fn from_str(s: &str) -> StdResult<Self, Self::Err> {
        match s {
            "sol" => Ok(Token::Sol),
            "hnt" => Ok(Token::Hnt),
            "mobile" => Ok(Token::Mobile),
            "iot" => Ok(Token::Iot),
            "dc" => Ok(Token::Dc),
            _ => Err(TokenError::InvalidToken(s.to_string())),
        }
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

    fn from_allowed(s: &str, allowed: &[Self]) -> StdResult<Self, TokenError> {
        let result = Self::from_str(s)?;
        if !allowed.contains(&result) {
            return Err(TokenError::InvalidToken(s.to_string()));
        }
        Ok(result)
    }

    pub fn transferrable_value_parser(s: &str) -> StdResult<Self, TokenError> {
        Self::from_allowed(s, &[Self::Iot, Self::Mobile, Self::Hnt, Self::Sol])
    }

    pub fn pricekey_value_parser(s: &str) -> StdResult<Self, TokenError> {
        Self::from_allowed(s, &[Self::Iot, Self::Mobile, Self::Hnt])
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
    #[serde(serialize_with = "crate::token::serde_amount_value")]
    pub amount: TokenAmount,
}

#[derive(Debug, serde::Serialize)]
pub struct TokenBalanceMap(HashMap<Token, TokenBalance>);

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

#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
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

pub fn serde_amount_value<S>(value: &TokenAmount, serializer: S) -> StdResult<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if value.token.decimals() == 0 {
        serializer.serialize_u64(value.amount)
    } else {
        serializer.serialize_f64(value.into())
    }
}

impl serde::Serialize for TokenAmount {
    fn serialize<S>(&self, serializer: S) -> StdResult<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut amount = serializer.serialize_struct("TokenAmount", 2)?;
        amount.serialize_field("token", &self.token)?;
        if self.token.decimals() == 0 {
            amount.serialize_field("amount", &self.amount)?;
        } else {
            amount.serialize_field("amount", &f64::from(self))?;
        }
        amount.end()
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
            Self::Iot => Some(&IOT_PRICE_KEY),
            Self::Mobile => Some(&MOBILE_PRICE_KEY),
            _ => None,
        }
    }

    pub fn price_feed(&self) -> Option<&price::FeedId> {
        match self {
            Self::Hnt => Some(&HNT_PRICE_FEED),
            Self::Iot => Some(&IOT_PRICE_FEED),
            Self::Mobile => Some(&MOBILE_PRICE_FEED),
            _ => None,
        }
    }

    pub fn amount(self, amount: u64) -> TokenAmount {
        TokenAmount::from_u64(self, amount)
    }

    pub fn to_balance(self, address: Pubkey, amount: u64) -> TokenBalance {
        TokenBalance {
            address,
            amount: TokenAmount::from_u64(self, amount),
        }
    }
}
