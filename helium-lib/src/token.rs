use crate::{
    anchor_lang::AccountDeserialize,
    anchor_spl, circuit_breaker,
    client::SolanaRpcClient,
    error::{DecodeError, Error},
    keypair::{serde_pubkey, Keypair, Pubkey},
    message, priority_fee,
    solana_sdk::{account::Account, signer::Signer},
    transaction::{mk_transaction, VersionedTransaction},
    TransactionOpts,
};
use chrono::{DateTime, Duration, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use itertools::Itertools;
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
    static ref USDC_MINT: Pubkey = Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
}

/// Number of Compute Units need to execute SetComputeUnitLimit and
/// ComputeBudget, together.
/// (Observed value: 450)
const SYS_PROGRAM_SETUP_CU: u32 = 600;

/// Number of Compute Units need to execute a System Program: Transfer
/// instruction.
/// (Actual value: 150)
const SYS_PROGRAM_TRANSFER_CU: u32 = 200;

/// Number of Compute Units needed to execute an SPL_CreateIdempotent
/// instruction in its worst case; the case in which it must actually create
/// an ATA.
/// (Actual value: 30998, observed on-chain 2025-01)
const SPL_CREATE_IDEMPOTENT_CU: u32 = 32000;

/// Number of Compute Units needed to execute an SPL_TransferChecked instruction.
/// (Actual value: 6199, observed on-chain 2025-02-09)
const SPL_TRANSFER_CHECKED_CU: u32 = 7000;

/// Number of Compute Units needed to execute an SPL CloseAccount instruction.
/// (Estimated value based on similar operations)
const SPL_CLOSE_ACCOUNT_CU: u32 = 3000;

pub async fn burn_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    token_amount: &TokenAmount,
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let ix = match token_amount.token.mint() {
        spl_mint if spl_mint == Token::Sol.mint() => {
            return Err(DecodeError::other("native token burn not supported").into());
        }
        spl_mint => {
            let token_account = token_amount.token.associated_token_adress(payer);
            anchor_spl::token::spl_token::instruction::burn_checked(
                &anchor_spl::token::spl_token::id(),
                &token_account,
                spl_mint,
                payer,
                &[payer],
                token_amount.amount,
                token_amount.token.decimals(),
            )?
        }
    };

    message::mk_message(client, &[ix], &opts.lut_addresses, payer).await
}

pub async fn burn<C: AsRef<SolanaRpcClient>>(
    client: &C,
    token_amount: &TokenAmount,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (msg, block_height) = burn_message(client, token_amount, &keypair.pubkey(), opts).await?;
    let txn = mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
}

pub async fn transfer_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    transfers: &[(Pubkey, TokenAmount)],
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let mut ixs = vec![];
    let mut ixs_accounts = vec![];
    let mut cu_budget: u32 = SYS_PROGRAM_SETUP_CU;
    for (payee, token_amount) in transfers {
        match token_amount.token.mint() {
            spl_mint if spl_mint == Token::Sol.mint() => {
                let ix = solana_system_interface::instruction::transfer(
                    payer,
                    payee,
                    token_amount.amount,
                );
                ixs_accounts.extend_from_slice(&ix.accounts);
                ixs.push(ix);
                cu_budget += SYS_PROGRAM_TRANSFER_CU;
            }
            spl_mint => {
                let source_pubkey = token_amount.token.associated_token_adress(payer);
                let destination_pubkey = token_amount.token.associated_token_adress(payee);
                let ix = spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                    payer,
                    payee,
                    spl_mint,
                    &anchor_spl::token::spl_token::id(),
                );
                ixs_accounts.extend_from_slice(&ix.accounts);
                ixs.push(ix);
                cu_budget += SPL_CREATE_IDEMPOTENT_CU;

                let ix = anchor_spl::token::spl_token::instruction::transfer_checked(
                    &anchor_spl::token::spl_token::id(),
                    &source_pubkey,
                    token_amount.token.mint(),
                    &destination_pubkey,
                    payer,
                    &[],
                    token_amount.amount,
                    token_amount.token.decimals(),
                )?;
                ixs_accounts.extend_from_slice(&ix.accounts);
                ixs.push(ix);
                cu_budget += SPL_TRANSFER_CHECKED_CU;
            }
        }
    }
    let final_ixs = &[
        &[
            priority_fee::compute_budget_instruction(cu_budget),
            priority_fee::compute_price_instruction_for_accounts(
                client,
                &ixs_accounts,
                opts.fee_range(),
            )
            .await?,
        ],
        ixs.as_slice(),
    ]
    .concat();
    message::mk_message(client, final_ixs, &opts.lut_addresses, payer).await
}

pub async fn transfer<C: AsRef<SolanaRpcClient>>(
    client: &C,
    transfers: &[(Pubkey, TokenAmount)],
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (msg, block_height) = transfer_message(client, transfers, &keypair.pubkey(), opts).await?;
    let txn = mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
}

/// Build a message to close multiple accounts and return funds to destination.
///
/// Supports both SPL token accounts and the owner's system account:
/// - SPL token accounts (any account != owner): closed via `spl_token::close_account`.
/// Must have zero token balance (caller is responsible for validation).
/// - System account (owner's own pubkey in the list): drained via `system_program::transfer`
/// of the full SOL balance to destination.
///
/// When the owner's system account is included, `fee_payer` must differ from `owner`
/// so the system account can be fully drained to zero.
pub async fn close_accounts_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    accounts: &[Pubkey],
    destination: &Pubkey,
    owner: &Pubkey,
    fee_payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    if accounts.is_empty() {
        return Err(DecodeError::other("no accounts to close").into());
    }
    if accounts.contains(owner) && fee_payer == owner {
        return Err(DecodeError::other(
            "fee_payer must differ from owner when closing the system account",
        )
        .into());
    }

    // Build SPL token close instructions
    let spl_ixs: Vec<_> = accounts
        .iter()
        .filter(|account| *account != owner)
        .map(|account| {
            anchor_spl::token::spl_token::instruction::close_account(
                &anchor_spl::token::spl_token::id(),
                account,
                destination,
                owner,
                &[],
            )
            .map_err(Error::from)
        })
        .try_collect()?;

    // Build system transfer if owner's system account is in the list
    let system_ix = if accounts.contains(owner) {
        let balance = client.as_ref().get_balance(owner).await?;
        (balance > 0)
            .then(|| solana_system_interface::instruction::transfer(owner, destination, balance))
    } else {
        None
    };

    let cu_budget = SYS_PROGRAM_SETUP_CU
        + spl_ixs.len() as u32 * SPL_CLOSE_ACCOUNT_CU
        + system_ix.as_ref().map_or(0, |_| SYS_PROGRAM_TRANSFER_CU);
    let ixs = spl_ixs.into_iter().chain(system_ix).collect_vec();

    let final_ixs = &[
        &[
            priority_fee::compute_budget_instruction(cu_budget),
            priority_fee::compute_price_instruction_for_instructions(
                client,
                &ixs,
                opts.fee_range(),
            )
            .await?,
        ],
        ixs.as_slice(),
    ]
    .concat();

    message::mk_message(client, final_ixs, &opts.lut_addresses, fee_payer).await
}

/// Close multiple accounts in a single transaction.
///
/// Supports both SPL token accounts and the owner's system account.
/// When closing the system account (owner's pubkey in the list), provide a
/// separate `fee_payer` so the owner's account can be fully drained to zero.
/// SPL token accounts must have zero balance.
pub async fn close_accounts<C: AsRef<SolanaRpcClient>>(
    client: &C,
    accounts: &[Pubkey],
    destination: &Pubkey,
    owner: &Keypair,
    fee_payer: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (msg, block_height) = close_accounts_message(
        client,
        accounts,
        destination,
        &owner.pubkey(),
        &fee_payer.pubkey(),
        opts,
    )
    .await?;
    let signers: Vec<&Keypair> = if fee_payer.pubkey() == owner.pubkey() {
        vec![owner]
    } else {
        vec![fee_payer, owner]
    };
    let txn = mk_transaction(msg, &signers)?;
    Ok((txn, block_height))
}

/// Close a single account and return funds to destination.
/// Supports both SPL token accounts and the owner's system account.
pub async fn close_account<C: AsRef<SolanaRpcClient>>(
    client: &C,
    account: &Pubkey,
    destination: &Pubkey,
    owner: &Keypair,
    fee_payer: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    close_accounts(client, &[*account], destination, owner, fee_payer, opts).await
}

pub async fn balance_for_address<C: AsRef<SolanaRpcClient>>(
    client: &C,
    pubkey: &Pubkey,
) -> Result<Option<TokenBalance>, Error> {
    let solana_client = client.as_ref();
    to_token_balance(
        *pubkey,
        solana_client
            .get_account_with_commitment(pubkey, solana_client.commitment())
            .await?
            .value,
    )
}

fn to_token_balance(pubkey: Pubkey, value: Option<Account>) -> Result<Option<TokenBalance>, Error> {
    match value {
        Some(account) if account.owner == anchor_spl::token::spl_token::ID => {
            let token_account =
                anchor_spl::token::TokenAccount::try_deserialize(&mut account.data.as_slice())?;
            let token = Token::from_mint(token_account.mint)
                .ok_or_else(|| DecodeError::other("Invalid mint"))?;
            Ok(Some(token.to_balance(pubkey, token_account.amount)))
        }
        Some(account) => Ok(Some(Token::Sol.to_balance(pubkey, account.lamports))),
        None => Ok(None),
    }
}

pub async fn balance_for_addresses<C: AsRef<SolanaRpcClient>>(
    client: &C,
    pubkeys: &[Pubkey],
) -> Result<Vec<Option<TokenBalance>>, Error> {
    let maybe_accounts = stream::iter(pubkeys.to_owned())
        .chunks(100)
        .map(|key_chunk| async move {
            client
                .as_ref()
                .get_multiple_accounts(key_chunk.as_slice())
                .await
        })
        .buffered(5)
        .try_collect::<Vec<Vec<Option<Account>>>>()
        .await?
        .into_iter()
        .flatten()
        .collect_vec();
    pubkeys
        .iter()
        .zip_eq(maybe_accounts)
        .map(|(pubkey, maybe_account)| to_token_balance(*pubkey, maybe_account))
        .try_collect()
}

pub mod price {
    use super::*;
    use crate::programs::helium_entity_manager::accounts::PriceUpdateV2;
    use rust_decimal::prelude::*;

    pub type FeedId = [u8; 32];
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
        let mut feed_id = [0; 32];
        hex::decode_to_slice(str, &mut feed_id).map_err(|_| PriceError::InvalidFeed)?;
        Ok(feed_id)
    }

    pub async fn get_with_max_age<C: AsRef<SolanaRpcClient>>(
        client: &C,
        token: Token,
        max_age: Duration,
    ) -> Result<Price, Error> {
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
    Usdc,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Token::Sol => "sol",
            Token::Hnt => "hnt",
            Token::Mobile => "mobile",
            Token::Iot => "iot",
            Token::Dc => "dc",
            Token::Usdc => "usdc",
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
            "usdc" => Ok(Token::Usdc),
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
            mint if mint == *USDC_MINT => Token::Usdc,
            _ => return None,
        };

        Some(token)
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Hnt,
            Self::Iot,
            Self::Mobile,
            Self::Dc,
            Self::Sol,
            Self::Usdc,
        ]
    }

    /// Tokens that can be transferred (excludes DC)
    pub fn transferrable() -> Vec<Self> {
        vec![Self::Hnt, Self::Iot, Self::Mobile, Self::Sol, Self::Usdc]
    }

    fn from_allowed(s: &str, allowed: &[Self]) -> StdResult<Self, TokenError> {
        let result = Self::from_str(s)?;
        if !allowed.contains(&result) {
            return Err(TokenError::InvalidToken(s.to_string()));
        }
        Ok(result)
    }

    pub fn transferrable_value_parser(s: &str) -> StdResult<Self, TokenError> {
        Self::from_allowed(
            s,
            &[Self::Iot, Self::Mobile, Self::Hnt, Self::Sol, Self::Usdc],
        )
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
            &circuit_breaker::ID,
        );
        circuit_breaker
    }
}

#[derive(Debug, serde::Serialize, Default, Clone, Copy)]
pub struct TokenBalance {
    #[serde(with = "serde_pubkey")]
    pub address: Pubkey,
    pub amount: TokenAmount,
}

#[derive(Debug, serde::Serialize, Clone, Default)]
pub struct TokenBalanceMap(HashMap<Token, TokenBalance>);

impl AsRef<HashMap<Token, TokenBalance>> for TokenBalanceMap {
    fn as_ref(&self) -> &HashMap<Token, TokenBalance> {
        &self.0
    }
}

impl AsMut<HashMap<Token, TokenBalance>> for TokenBalanceMap {
    fn as_mut(&mut self) -> &mut HashMap<Token, TokenBalance> {
        &mut self.0
    }
}

impl From<Vec<Option<TokenBalance>>> for TokenBalanceMap {
    fn from(value: Vec<Option<TokenBalance>>) -> Self {
        Self(
            value
                .into_iter()
                .filter_map(|maybe_balance| {
                    maybe_balance.map(|balance| (balance.amount.token, balance))
                })
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

impl<'de> serde::Deserialize<'de> for TokenAmount {
    fn deserialize<D>(deserializer: D) -> StdResult<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Token,
            Amount,
        }

        struct TokenAmountVisitor;

        impl<'de> Visitor<'de> for TokenAmountVisitor {
            type Value = TokenAmount;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct TokenAmount")
            }

            fn visit_map<V>(self, mut map: V) -> StdResult<TokenAmount, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut token: Option<Token> = None;
                let mut amount_value: Option<serde_json::Value> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Token => {
                            if token.is_some() {
                                return Err(de::Error::duplicate_field("token"));
                            }
                            token = Some(map.next_value()?);
                        }
                        Field::Amount => {
                            if amount_value.is_some() {
                                return Err(de::Error::duplicate_field("amount"));
                            }
                            amount_value = Some(map.next_value::<serde_json::Value>()?);
                        }
                    }
                }

                let token = token.ok_or_else(|| de::Error::missing_field("token"))?;
                let amount_value =
                    amount_value.ok_or_else(|| de::Error::missing_field("amount"))?;

                let token_amount = if token.decimals() == 0 {
                    let amount = amount_value
                        .as_u64()
                        .ok_or_else(|| de::Error::custom("expected integer for 0-decimal token"))?;
                    TokenAmount::from_u64(token, amount)
                } else {
                    let amount = amount_value
                        .as_f64()
                        .ok_or_else(|| de::Error::custom("expected float for decimal token"))?;
                    TokenAmount::from_f64(token, amount)
                };

                Ok(token_amount)
            }
        }

        const FIELDS: &[&str] = &["token", "amount"];
        deserializer.deserialize_struct("TokenAmount", FIELDS, TokenAmountVisitor)
    }
}

impl Default for TokenAmount {
    fn default() -> Self {
        Self {
            token: Token::Sol,
            amount: 0,
        }
    }
}

impl TokenAmount {
    pub fn from_f64<T: Into<Token>>(token: T, amount: f64) -> Self {
        let token = token.into();
        let amount = (amount * 10_usize.pow(token.decimals().into()) as f64) as u64;
        Self { token, amount }
    }

    pub fn from_u64<T: Into<Token>>(token: T, amount: u64) -> Self {
        Self {
            token: token.into(),
            amount,
        }
    }
}

impl Token {
    pub fn decimals(&self) -> u8 {
        match self {
            Self::Hnt => 8,
            Self::Iot | Self::Mobile => 6,
            Self::Dc => 0,
            Self::Sol => 9,
            Self::Usdc => 6,
        }
    }

    pub fn mint(&self) -> &Pubkey {
        match self {
            Self::Hnt => &HNT_MINT,
            Self::Mobile => &MOBILE_MINT,
            Self::Iot => &IOT_MINT,
            Self::Dc => &DC_MINT,
            Self::Sol => &SOL_MINT,
            Self::Usdc => &USDC_MINT,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_amount_serde_roundtrip_zero_decimals() {
        let original = TokenAmount::from_u64(Token::Dc, 100);
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TokenAmount = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
        assert!(json.contains("\"amount\":100"));
    }

    #[test]
    fn test_token_amount_serde_roundtrip_with_decimals() {
        let original = TokenAmount::from_f64(Token::Hnt, 1.5);
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TokenAmount = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
        assert!(json.contains("\"amount\":1.5"));
    }

    #[test]
    fn test_token_balance_serialization() {
        let balance = TokenBalance {
            address: Pubkey::new_unique(),
            amount: TokenAmount::from_f64(Token::Mobile, 10.5),
        };
        let json = serde_json::to_string(&balance).unwrap();
        assert!(json.contains("\"amount\":{\"token\":\"mobile\",\"amount\":10.5}"));
    }

    #[test]
    fn test_token_amount_deserialize_invalid_type_for_zero_decimal() {
        let json = r#"{"token":"dc","amount":1.5}"#;
        let result = serde_json::from_str::<TokenAmount>(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("expected integer for 0-decimal token"));
    }

    #[test]
    fn test_token_amount_deserialize_invalid_type_for_decimal() {
        let json = r#"{"token":"hnt","amount":"invalid"}"#;
        let result = serde_json::from_str::<TokenAmount>(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("expected float for decimal token"));
    }

    #[test]
    fn test_token_amount_deserialize_missing_fields() {
        let json = r#"{"token":"hnt"}"#;
        let result = serde_json::from_str::<TokenAmount>(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing field `amount`"));

        let json = r#"{"amount":1.5}"#;
        let result = serde_json::from_str::<TokenAmount>(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing field `token`"));
    }
}
