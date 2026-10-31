use crate::{
    anchor_lang::AccountDeserialize,
    anchor_spl, circuit_breaker,
    client::SolanaRpcClient,
    error::{DecodeError, Error},
    keypair::{serde_pubkey, Pubkey},
    message,
    solana_sdk::{account::Account, signer::Signer},
    transaction::{mk_signed_transaction, VersionedTransaction},
    TransactionOpts,
};
use chrono::{DateTime, Duration, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use itertools::Itertools;
use std::{collections::HashMap, result::Result as StdResult, str::FromStr};

/// Errors from parsing or validating a token type.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Invalid token type: {0}")]
    InvalidToken(String),
}

lazy_static::lazy_static! {
    static ref HNT_MINT: Pubkey = Pubkey::from_str("hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux").unwrap();
    static ref HNT_PRICE_FEED: price::FeedId = price::feed_from_hex("649fdd7ec08e8e2a20f425729854e90293dcbe2376abc47197a14da6ff339756").unwrap();

    static ref MOBILE_MINT: Pubkey = Pubkey::from_str("mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6").unwrap();

    static ref IOT_MINT: Pubkey = Pubkey::from_str("iotEVVZLEywoTn1QdwNPddxPWszn3zFhEot3MfL9fns").unwrap();

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

/// Build the bare burn instruction (no compute-budget framing) for a
/// given owner. Native SOL burn is not supported.
pub fn burn_instruction(
    payer: &Pubkey,
    token_amount: &TokenAmount,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    match token_amount.token.mint() {
        spl_mint if spl_mint == Token::Sol.mint() => {
            Err(DecodeError::other("native token burn not supported").into())
        }
        spl_mint => {
            let token_account = token_amount.token.associated_token_address(payer);
            Ok(anchor_spl::token::spl_token::instruction::burn_checked(
                &anchor_spl::token::spl_token::id(),
                &token_account,
                spl_mint,
                payer,
                &[payer],
                token_amount.amount,
                token_amount.token.decimals(),
            )?)
        }
    }
}

pub async fn burn_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    token_amount: &TokenAmount,
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let ix = burn_instruction(payer, token_amount)?;
    message::mk_message(client, &[ix], &opts.lut_addresses, payer).await
}

/// Burns a token amount from the keypair's account, returning a signed transaction.
pub async fn burn<C: AsRef<SolanaRpcClient>>(
    client: &C,
    token_amount: &TokenAmount,
    keypair: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let msg = burn_message(client, token_amount, &keypair.pubkey(), opts).await?;
    mk_signed_transaction(msg, &[keypair])
}

/// Build the bare transfer instructions (no compute-budget framing) for
/// one or more recipients, with `payer` as the source authority. Used
/// directly by callers that don't want the compute-budget envelope —
/// e.g. wrapping the instructions into a Squads proposal where the
/// outer (proposer) transaction supplies its own compute budget.
pub fn transfer_instructions(
    payer: &Pubkey,
    transfers: &[(Pubkey, TokenAmount)],
) -> Result<Vec<solana_sdk::instruction::Instruction>, Error> {
    let mut ixs = Vec::new();
    for (payee, token_amount) in transfers {
        match token_amount.token.mint() {
            spl_mint if spl_mint == Token::Sol.mint() => {
                ixs.push(solana_system_interface::instruction::transfer(
                    payer,
                    payee,
                    token_amount.amount,
                ));
            }
            spl_mint => {
                let source_pubkey = token_amount.token.associated_token_address(payer);
                let destination_pubkey = token_amount.token.associated_token_address(payee);
                ixs.push(
                    spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                        payer,
                        payee,
                        spl_mint,
                        &anchor_spl::token::spl_token::id(),
                    ),
                );
                ixs.push(anchor_spl::token::spl_token::instruction::transfer_checked(
                    &anchor_spl::token::spl_token::id(),
                    &source_pubkey,
                    token_amount.token.mint(),
                    &destination_pubkey,
                    payer,
                    &[],
                    token_amount.amount,
                    token_amount.token.decimals(),
                )?);
            }
        }
    }
    Ok(ixs)
}

/// Builds a versioned message that transfers tokens to one or more recipients.
pub async fn transfer_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    transfers: &[(Pubkey, TokenAmount)],
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let ixs = transfer_instructions(payer, transfers)?;
    let cu_budget: u32 = SYS_PROGRAM_SETUP_CU
        + transfers
            .iter()
            .map(|(_, ta)| {
                if ta.token.mint() == Token::Sol.mint() {
                    SYS_PROGRAM_TRANSFER_CU
                } else {
                    SPL_CREATE_IDEMPOTENT_CU + SPL_TRANSFER_CHECKED_CU
                }
            })
            .sum::<u32>();
    message::mk_budgeted_message(client, cu_budget, &ixs, payer, opts).await
}

/// Transfer tokens to one or more recipients, returning a signed transaction.
pub async fn transfer<C: AsRef<SolanaRpcClient>>(
    client: &C,
    transfers: &[(Pubkey, TokenAmount)],
    keypair: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let msg = transfer_message(client, transfers, &keypair.pubkey(), opts).await?;
    mk_signed_transaction(msg, &[keypair])
}

/// Builds a message to close multiple accounts and return funds to destination.
///
/// Supports both SPL token accounts and the owner's system account:
/// - SPL token accounts (any account != owner): closed via `spl_token::close_account`.
///   Must have zero token balance (caller is responsible for validation).
/// - System account (owner's own pubkey in the list): drained via `system_program::transfer`
///   of the full SOL balance to destination.
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

    message::mk_budgeted_message(client, cu_budget, &ixs, fee_payer, opts).await
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
    owner: &(dyn Signer + Sync),
    fee_payer: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let msg = close_accounts_message(
        client,
        accounts,
        destination,
        &owner.pubkey(),
        &fee_payer.pubkey(),
        opts,
    )
    .await?;
    let signers: Vec<&(dyn Signer + Sync)> = if fee_payer.pubkey() == owner.pubkey() {
        vec![owner]
    } else {
        vec![fee_payer, owner]
    };
    mk_signed_transaction(msg, &signers)
}

/// Close a single account and return funds to destination.
/// Supports both SPL token accounts and the owner's system account.
pub async fn close_account<C: AsRef<SolanaRpcClient>>(
    client: &C,
    account: &Pubkey,
    destination: &Pubkey,
    owner: &(dyn Signer + Sync),
    fee_payer: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    close_accounts(client, &[*account], destination, owner, fee_payer, opts).await
}

/// Fetches the token balance for a single account address.
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

/// Fetches token balances for multiple account addresses in batched RPC calls.
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

/// Token prices. HNT is the only token with a maintained feed, read from the
/// oracle the data-credits program pins.
pub mod price {
    use super::*;
    use crate::{
        dao::Dao,
        keypair::pubkey,
        programs::{
            data_credits::accounts::DataCreditsV0,
            helium_entity_manager::{
                accounts::PriceUpdateV2,
                types::{PriceFeedMessage, VerificationLevel},
            },
        },
    };
    use futures::TryFutureExt;
    use rust_decimal::prelude::*;
    use std::future::Future;

    /// Raw 32-byte Pyth price feed identifier.
    pub type FeedId = [u8; 32];
    /// Number of Data Credits per USD.
    pub const DC_PER_USD: i64 = 100_000;

    /// The Pyth receiver that must own the HNT price account the
    /// data-credits program pins. Its `load_hnt_price_oracle` refuses any
    /// other owner, the legacy receiver
    /// (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`) included.
    const PYTH_RECEIVER_ID: Pubkey = pubkey!("rec2HHDDnjLfj4kE7VyEtFA1HPGQLK33259532cRyHp");

    /// Errors from fetching or validating a token price.
    #[derive(Debug, thiserror::Error)]
    pub enum PriceError {
        #[error("invalid or unsupported token: {0}")]
        InvalidToken(Token),
        #[error("{0} has no maintained price feed")]
        NoFeed(Token),
        #[error("price account owned by {0}, not the Pyth receiver")]
        InvalidOwner(Pubkey),
        #[error("price update is only partially verified")]
        PartialVerification,
        #[error("invalid price feed")]
        InvalidFeed,
        #[error("price too old")]
        TooOld,
        #[error("price not positive")]
        NotPositive,
        #[error("invalid price timestamp: {0}")]
        InvalidTimestamp(i64),
        #[error("unsupported positive price exponent")]
        PositiveExponent,
    }

    /// A token price snapshot from the Pyth oracle.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct Price {
        pub timestamp: DateTime<Utc>,
        pub price: Decimal,
        pub token: Token,
    }

    pub fn feed_from_hex(str: &str) -> Result<FeedId, PriceError> {
        let mut feed_id = [0; 32];
        hex::decode_to_slice(str, &mut feed_id).map_err(|_| PriceError::InvalidFeed)?;
        Ok(feed_id)
    }

    /// Fetch the current price for a token, rejecting prices older than `max_age`.
    ///
    /// Only HNT has a price: it is read from the account the data-credits
    /// program prices DC mints from (see [`hnt_message`]), and the value
    /// returned is the optimistic end of the EMA's confidence interval,
    /// `ema_price + 2 * ema_conf`. MOBILE and IOT have no maintained price
    /// feed and fail with [`PriceError::NoFeed`].
    pub async fn get_with_max_age<C: AsRef<SolanaRpcClient>>(
        client: &C,
        token: Token,
        max_age: Duration,
    ) -> Result<Price, Error> {
        match token {
            Token::Hnt => {}
            Token::Mobile | Token::Iot => return Err(PriceError::NoFeed(token).into()),
            other => return Err(PriceError::InvalidToken(other).into()),
        }
        let message = hnt_message(client, max_age).await?;
        Ok(optimistic_price(&message)?)
    }

    /// Fetch the current price for a token with a default 10-minute max age.
    pub async fn get<C: AsRef<SolanaRpcClient>>(client: &C, token: Token) -> Result<Price, Error> {
        get_with_max_age(client, token, Duration::minutes(10)).await
    }

    /// Fetch the HNT price message the data-credits program prices DC mints
    /// from.
    ///
    /// The account is the one `DataCreditsV0.hnt_price_oracle` names. This
    /// applies the data-credits program's owner, verification, feed, age and
    /// positive-EMA checks, and also refuses a positive exponent. Age is
    /// measured against the local clock and the caller's `max_age` (the
    /// program allows 10 minutes).
    ///
    /// The program mints at the conservative end, `ema_price - 2 * ema_conf`.
    /// That value can be zero or negative after these checks, so a caller
    /// that uses it must refuse a non-positive result.
    pub async fn hnt_message<C: AsRef<SolanaRpcClient>>(
        client: &C,
        max_age: Duration,
    ) -> Result<PriceFeedMessage, Error> {
        let client = client.as_ref();
        load_hnt_message(
            |key| async move { client.get_account(&key).map_err(Error::from).await },
            Utc::now().timestamp(),
            max_age,
        )
        .await
    }

    async fn load_hnt_message<F, Fut>(
        fetch: F,
        now: i64,
        max_age: Duration,
    ) -> Result<PriceFeedMessage, Error>
    where
        F: Fn(Pubkey) -> Fut,
        Fut: Future<Output = Result<Account, Error>>,
    {
        let dc = fetch(Dao::dc_key()).await?;
        let DataCreditsV0 {
            hnt_price_oracle, ..
        } = DataCreditsV0::try_deserialize(&mut dc.data.as_slice())?;
        let oracle = fetch(hnt_price_oracle).await?;
        checked_hnt_message(&oracle, now, max_age)
    }

    fn checked_hnt_message(
        account: &Account,
        now: i64,
        max_age: Duration,
    ) -> Result<PriceFeedMessage, Error> {
        if account.owner != PYTH_RECEIVER_ID {
            return Err(PriceError::InvalidOwner(account.owner).into());
        }
        let PriceUpdateV2 {
            verification_level,
            price_message,
            ..
        } = PriceUpdateV2::try_deserialize(&mut account.data.as_slice())?;
        if !matches!(verification_level, VerificationLevel::Full) {
            return Err(PriceError::PartialVerification.into());
        }
        if price_message.feed_id != *HNT_PRICE_FEED {
            return Err(PriceError::InvalidFeed.into());
        }
        if price_message
            .publish_time
            .saturating_add(max_age.num_seconds())
            < now
        {
            return Err(PriceError::TooOld.into());
        }
        if price_message.ema_price <= 0 {
            return Err(PriceError::NotPositive.into());
        }
        if price_message.exponent > 0 {
            return Err(PriceError::PositiveExponent.into());
        }
        Ok(price_message)
    }

    fn optimistic_price(message: &PriceFeedMessage) -> Result<Price, PriceError> {
        let scale = message.exponent.unsigned_abs();
        // Add the confidence interval to the price to get the most optimistic price:
        let mut price = Decimal::new(message.ema_price, scale)
            + Decimal::new(message.ema_conf as i64, scale) * Decimal::new(2, 0);
        // ensure we use only up to 6 decimals, this rounds using `MidpointAwayFromZero`
        price.rescale(6);
        let timestamp = DateTime::from_timestamp(message.publish_time, 0)
            .ok_or(PriceError::InvalidTimestamp(message.publish_time))?;
        Ok(Price {
            timestamp,
            price,
            token: Token::Hnt,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::anchor_lang::AccountSerialize;
        use std::sync::{Arc, Mutex};

        const NOW: i64 = 1_800_000_000;
        const LEGACY_RECEIVER_ID: Pubkey = pubkey!("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");

        fn message() -> PriceFeedMessage {
            PriceFeedMessage {
                feed_id: *HNT_PRICE_FEED,
                price: 150_000_000,
                conf: 100_000,
                exponent: -8,
                publish_time: NOW - 3,
                prev_publish_time: NOW - 4,
                ema_price: 150_000_000,
                ema_conf: 200_000,
            }
        }

        fn price_account(
            owner: Pubkey,
            verification_level: VerificationLevel,
            price_message: PriceFeedMessage,
        ) -> Account {
            let update = PriceUpdateV2 {
                write_authority: Pubkey::new_unique(),
                verification_level,
                price_message,
                posted_slot: 1,
            };
            let mut data = Vec::new();
            update
                .try_serialize(&mut data)
                .expect("serialize price update");
            Account {
                lamports: 1,
                data,
                owner,
                executable: false,
                rent_epoch: 0,
            }
        }

        fn valid(price_message: PriceFeedMessage) -> Account {
            price_account(PYTH_RECEIVER_ID, VerificationLevel::Full, price_message)
        }

        fn check(account: &Account) -> Result<PriceFeedMessage, Error> {
            checked_hnt_message(account, NOW, Duration::minutes(10))
        }

        #[test]
        fn accepts_a_fresh_fully_verified_hnt_update() {
            let checked = check(&valid(message())).expect("valid update");
            assert_eq!(checked.ema_price, 150_000_000);
            assert_eq!(checked.ema_conf, 200_000);
        }

        #[test]
        fn refuses_the_legacy_receiver() {
            let account = price_account(LEGACY_RECEIVER_ID, VerificationLevel::Full, message());
            assert!(matches!(
                check(&account),
                Err(Error::Price(PriceError::InvalidOwner(owner))) if owner == LEGACY_RECEIVER_ID
            ));
        }

        #[test]
        fn refuses_a_partially_verified_update() {
            let account = price_account(
                PYTH_RECEIVER_ID,
                VerificationLevel::Partial { num_signatures: 5 },
                message(),
            );
            assert!(matches!(
                check(&account),
                Err(Error::Price(PriceError::PartialVerification))
            ));
        }

        #[test]
        fn refuses_another_feed() {
            let account = valid(PriceFeedMessage {
                feed_id: [7; 32],
                ..message()
            });
            assert!(matches!(
                check(&account),
                Err(Error::Price(PriceError::InvalidFeed))
            ));
        }

        #[test]
        fn accepts_an_update_exactly_max_age_old_and_refuses_one_older() {
            let at_limit = valid(PriceFeedMessage {
                publish_time: NOW - 600,
                ..message()
            });
            check(&at_limit).expect("update at the age limit");
            let stale = valid(PriceFeedMessage {
                publish_time: NOW - 601,
                ..message()
            });
            assert!(matches!(
                check(&stale),
                Err(Error::Price(PriceError::TooOld))
            ));
        }

        #[test]
        fn refuses_a_non_positive_ema_price() {
            for ema_price in [0, -1] {
                let account = valid(PriceFeedMessage {
                    ema_price,
                    ..message()
                });
                assert!(matches!(
                    check(&account),
                    Err(Error::Price(PriceError::NotPositive))
                ));
            }
        }

        #[test]
        fn refuses_a_positive_exponent() {
            let account = valid(PriceFeedMessage {
                exponent: 1,
                ..message()
            });
            assert!(matches!(
                check(&account),
                Err(Error::Price(PriceError::PositiveExponent))
            ));
        }

        #[test]
        fn optimistic_price_adds_twice_the_ema_confidence() {
            let price = optimistic_price(&message()).expect("price");
            // 1.5 + 2 * 0.002
            assert_eq!(price.price, Decimal::new(1_504_000, 6));
            assert_eq!(price.price.scale(), 6);
            assert_eq!(price.token, Token::Hnt);
            assert_eq!(
                price.timestamp,
                DateTime::from_timestamp(NOW - 3, 0).expect("timestamp")
            );
        }

        #[test]
        fn accepts_a_zero_exponent() {
            let checked = check(&valid(PriceFeedMessage {
                exponent: 0,
                ..message()
            }))
            .expect("zero exponent");
            assert_eq!(checked.exponent, 0);
        }

        /// A client on the discard port, which is not expected to answer: a
        /// token that reached the network fails with a transport error, not
        /// the refusal a test asserts.
        fn unreachable_client() -> Arc<SolanaRpcClient> {
            Arc::new(SolanaRpcClient::new("http://127.0.0.1:9".to_string()))
        }

        #[tokio::test]
        async fn tokens_outside_hnt_mobile_and_iot_are_invalid_without_a_fetch() {
            let client = unreachable_client();
            for token in [Token::Dc, Token::Sol, Token::Usdc] {
                assert!(matches!(
                    get_with_max_age(&client, token, Duration::minutes(10)).await,
                    Err(Error::Price(PriceError::InvalidToken(refused))) if refused == token
                ));
            }
        }

        #[tokio::test]
        async fn mobile_and_iot_have_no_price_feed() {
            let client = unreachable_client();
            for token in [Token::Mobile, Token::Iot] {
                assert!(matches!(
                    get(&client, token).await,
                    Err(Error::Price(PriceError::NoFeed(refused))) if refused == token
                ));
            }
        }

        #[tokio::test]
        async fn hnt_resolves_through_the_data_credits_pin() {
            let pinned = Pubkey::new_unique();
            let dc = DataCreditsV0 {
                dc_mint: *Token::Dc.mint(),
                hnt_mint: *Token::Hnt.mint(),
                authority: Pubkey::new_unique(),
                hnt_price_oracle: pinned,
                data_credits_bump: 255,
                account_payer: Pubkey::new_unique(),
                account_payer_bump: 255,
            };
            let mut dc_data = Vec::new();
            dc.try_serialize(&mut dc_data)
                .expect("serialize data credits");
            let accounts: HashMap<Pubkey, Account> = [
                (
                    Dao::dc_key(),
                    Account {
                        lamports: 1,
                        data: dc_data,
                        owner: crate::data_credits::ID,
                        executable: false,
                        rent_epoch: 0,
                    },
                ),
                (pinned, valid(message())),
            ]
            .into_iter()
            .collect();
            let requested = Mutex::new(Vec::new());
            let fetch = |key: Pubkey| {
                requested.lock().expect("lock requested").push(key);
                let account = accounts
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| DecodeError::other("no such account").into());
                async move { account }
            };

            let checked = load_hnt_message(fetch, NOW, Duration::minutes(10))
                .await
                .expect("resolve through the pin");

            assert_eq!(checked.ema_price, 150_000_000);
            assert_eq!(
                *requested.lock().expect("lock requested"),
                vec![Dao::dc_key(), pinned]
            );
        }
    }
}

/// Helium ecosystem tokens on Solana.
#[derive(
    Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Copy, Hash, PartialOrd, Ord,
)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum Token {
    /// Native Solana token (lamports).
    Sol,
    /// Helium Network Token -- the primary governance and staking token.
    Hnt,
    /// MOBILE sub-DAO token for 5G/cellular coverage.
    Mobile,
    /// IOT sub-DAO token for LoRaWAN coverage.
    Iot,
    /// Data Credits -- non-transferable, burned to pay for network usage.
    Dc,
    /// USD Coin stablecoin.
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
    /// Look up a token by its SPL mint address, returning `None` for unknown mints.
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

    /// All supported token types.
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

    /// Parses the only token with a price, HNT.
    pub fn hnt_value_parser(s: &str) -> StdResult<Self, TokenError> {
        Self::from_allowed(s, &[Self::Hnt])
    }

    /// Derive the associated token account address for a wallet. Returns the wallet itself for SOL.
    pub fn associated_token_address(&self, address: &Pubkey) -> Pubkey {
        match self {
            Self::Sol => *address,
            _ => spl_associated_token_account::get_associated_token_address(address, self.mint()),
        }
    }

    /// Derive associated token account addresses for all supported tokens.
    pub fn associated_token_addresses(address: &Pubkey) -> Vec<Pubkey> {
        Self::all()
            .iter()
            .map(|token| token.associated_token_address(address))
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

/// A token balance at a specific on-chain account address.
#[derive(Debug, serde::Serialize, Default, Clone, Copy)]
pub struct TokenBalance {
    #[serde(with = "serde_pubkey")]
    pub address: Pubkey,
    pub amount: TokenAmount,
}

/// Map of token type to balance, used for multi-token wallet summaries.
///
/// Serializes as a flat `{ "<token>": <amount> }` object. Amounts follow the
/// same numeric convention as [`TokenAmount`]: integer for 0-decimal tokens
/// (DC), floating-point for the rest.
#[derive(Debug, Clone, Default)]
pub struct TokenBalanceMap(HashMap<Token, TokenBalance>);

impl serde::Serialize for TokenBalanceMap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> StdResult<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (token, balance) in &self.0 {
            if token.decimals() == 0 {
                map.serialize_entry(&token.to_string(), &balance.amount.amount)?;
            } else {
                map.serialize_entry(&token.to_string(), &f64::from(&balance.amount))?;
            }
        }
        map.end()
    }
}

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

/// A token amount stored as raw integer units (e.g. lamports for SOL, bones for HNT).
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
                    TokenAmount::from_f64(token, amount).map_err(de::Error::custom)?
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
    /// Largest integer an f64 represents exactly (2^53). Scaled amounts
    /// past this would silently lose base units in the cast.
    const MAX_F64_AMOUNT: f64 = (1u64 << f64::MANTISSA_DIGITS) as f64;

    /// Create a token amount from a human-readable decimal value (e.g. `1.5` HNT).
    ///
    /// Rejects negative, NaN, and infinite values, and values whose
    /// base-unit form exceeds f64's exact integer range, all of which
    /// the previous unchecked cast silently turned into a different
    /// on-chain amount.
    pub fn from_f64<T: Into<Token>>(token: T, amount: f64) -> Result<Self, DecodeError> {
        let token = token.into();
        if !amount.is_finite() || amount < 0.0 {
            return Err(DecodeError::other(format!(
                "invalid {token} amount: {amount}"
            )));
        }
        let scaled = amount * 10_usize.pow(token.decimals().into()) as f64;
        if scaled > Self::MAX_F64_AMOUNT {
            return Err(DecodeError::other(format!(
                "{token} amount {amount} is too large to represent exactly"
            )));
        }
        Ok(Self {
            token,
            amount: scaled as u64,
        })
    }

    /// Create a token amount from raw integer units (e.g. lamports, bones).
    pub fn from_u64<T: Into<Token>>(token: T, amount: u64) -> Self {
        Self {
            token: token.into(),
            amount,
        }
    }
}

impl Token {
    /// Number of decimal places for this token (e.g. 8 for HNT, 9 for SOL, 0 for DC).
    pub fn decimals(&self) -> u8 {
        match self {
            Self::Hnt => 8,
            Self::Iot | Self::Mobile => 6,
            Self::Dc => 0,
            Self::Sol => 9,
            Self::Usdc => 6,
        }
    }

    /// The SPL mint address for this token (system program ID for SOL).
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
    fn hnt_value_parser_accepts_hnt_alone() {
        assert_eq!(Token::hnt_value_parser("hnt").expect("hnt"), Token::Hnt);
        for other in ["mobile", "iot"] {
            assert!(matches!(
                Token::hnt_value_parser(other),
                Err(TokenError::InvalidToken(refused)) if refused == other
            ));
        }
    }

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
        let original = TokenAmount::from_f64(Token::Hnt, 1.5).expect("hnt token amount");
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TokenAmount = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
        assert!(json.contains("\"amount\":1.5"));
    }

    #[test]
    fn test_token_amount_from_f64_rejects_invalid() {
        for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                TokenAmount::from_f64(Token::Hnt, bad).is_err(),
                "expected rejection of {bad}"
            );
        }
        // 2^53 base units of HNT (8 decimals) is ~90M HNT; past that the
        // f64 cast would silently lose base units.
        assert!(TokenAmount::from_f64(Token::Hnt, 100_000_000.0).is_err());
        assert!(TokenAmount::from_f64(Token::Hnt, 0.0).expect("zero").amount == 0);
    }

    #[test]
    fn test_token_balance_serialization() {
        let balance = TokenBalance {
            address: Pubkey::new_unique(),
            amount: TokenAmount::from_f64(Token::Mobile, 10.5).expect("mobile token amount"),
        };
        let json = serde_json::to_string(&balance).unwrap();
        assert!(json.contains("\"amount\":{\"token\":\"mobile\",\"amount\":10.5}"));
    }

    #[test]
    fn test_token_balance_map_serialization_is_flat() {
        let address = Pubkey::new_unique();
        let mut map = TokenBalanceMap::default();
        map.as_mut().insert(
            Token::Hnt,
            Token::Hnt.to_balance(address, 150_000_000), // 1.5 HNT (8 decimals)
        );
        map.as_mut().insert(
            Token::Dc,
            Token::Dc.to_balance(address, 100), // 100 DC (0 decimals)
        );
        let value: serde_json::Value = serde_json::to_value(&map).unwrap();
        assert_eq!(value.get("hnt"), Some(&serde_json::json!(1.5)));
        assert_eq!(value.get("dc"), Some(&serde_json::json!(100)));
        // Per-ATA address and nested amount struct should not appear.
        let json = serde_json::to_string(&value).unwrap();
        assert!(!json.contains("\"address\""));
        assert!(!json.contains("\"token\":"));
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
