//! Checks that a Jupiter order and the transaction it carries do nothing
//! beyond the swap the caller asked for, before the taker signs it.
//!
//! The taker is the fee payer and the only signer, so a transaction it signs
//! can spend anything the taker holds. Three layers decide whether it may:
//!
//! 1. [`check_order`]: the order names the requested mints, amount,
//!    slippage, taker and an exact-in swap, quotes a non-zero output, and
//!    its minimum output is at least the caller's `min_output` and no lower
//!    than the slippage allows on the quote.
//! 2. [`check_instructions`]: every top-level instruction is one of the
//!    forms a Jupiter aggregator route emits (compute budget within caps,
//!    the taker's own associated token accounts, wrapping and unwrapping the
//!    taker's SOL, and the Jupiter aggregator's exact-in routes, whose own
//!    arguments must spend `amount`, enforce at least the order's minimum
//!    output on chain, deliver to the taker's output account and take a
//!    bounded platform fee).
//! 3. [`check_effects`]: a simulation of the unsigned transaction shows the
//!    taker spending exactly `amount` of the input mint, receiving at least
//!    the order's minimum output, losing no other token, paying no more SOL
//!    than the transaction's own fee, and every taker token account keeping
//!    its owner, delegate and close authority. A transaction can only modify
//!    accounts it lists as writable, so comparing every writable account
//!    before and after covers what the Jupiter program does through
//!    cross-program invocations, which no static check can see.
//!
//! The taker signs only after all three pass, so a refused transaction is
//! never signed and the RPC node that simulates it never holds a signed copy.

use super::{JupiterError, OrderResponse};
use crate::{
    client::SolanaRpcClient,
    keypair::Pubkey,
    solana_client::rpc_config::{
        RpcSimulateTransactionAccountsConfig, RpcSimulateTransactionConfig,
    },
    solana_sdk::{
        account::Account,
        hash::Hash,
        message::VersionedMessage,
        program_pack::Pack,
        pubkey,
        signature::Signature,
        signer::Signer,
        transaction::{TransactionError, VersionedTransaction},
    },
};
use anchor_client::solana_account_decoder::UiAccountEncoding;
use anchor_spl::token::spl_token::{
    self, native_mint,
    state::{Account as TokenAccount, Multisig},
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use solana_address_lookup_table_interface::{
    program as address_lookup_table, state::AddressLookupTable,
};
use solana_compute_budget_interface as compute_budget;
use solana_system_interface::program as system_program;
use solana_transaction_status::UiTransactionEncoding;
use spl_associated_token_account::{
    get_associated_token_address, get_associated_token_address_with_program_id,
};
use std::collections::{BTreeSet, HashMap};

/// Jupiter's aggregator program, which every `metis` route invokes.
pub(crate) const JUPITER_AGGREGATOR_V6: Pubkey =
    pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4");
/// The Token-2022 program, which associated token accounts may belong to.
const TOKEN_2022: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
/// The highest compute unit limit a swap transaction may request (the
/// protocol maximum per transaction).
pub const MAX_COMPUTE_UNIT_LIMIT: u32 = 1_400_000;
/// The highest compute unit price a swap transaction may set, in
/// micro-lamports. At the maximum limit this caps the priority fee at
/// 2,800,000 lamports (0.0028 SOL).
pub const MAX_COMPUTE_UNIT_PRICE: u64 = 2_000_000;
/// The base fee per signature, in lamports.
const LAMPORTS_PER_SIGNATURE: u64 = 5_000;
const MICRO_LAMPORTS_PER_LAMPORT: u128 = 1_000_000;
/// `AccountType::Account`, the byte Token-2022 writes after the base layout
/// of a token account that carries extensions.
const TOKEN_2022_ACCOUNT_TYPE: u8 = 2;
/// Associated Token Account program `CreateIdempotent`.
const ATA_CREATE_IDEMPOTENT: &[u8] = &[1];
/// SPL Token `SyncNative`.
const TOKEN_SYNC_NATIVE: &[u8] = &[17];
/// SPL Token `CloseAccount`.
const TOKEN_CLOSE_ACCOUNT: &[u8] = &[9];
/// System program `Transfer`, the bincode tag before its `u64` lamports.
const SYSTEM_TRANSFER_TAG: [u8; 4] = [2, 0, 0, 0];
/// Jupiter aggregator `route_v2` and `shared_accounts_route_v2`, the
/// exact-in routes the v2 order endpoint builds. Both carry `in_amount: u64`,
/// `quoted_out_amount: u64` and `slippage_bps: u16` at a fixed offset after
/// the discriminator (`shared_accounts_route_v2` first has an `id: u8`), and
/// the program refuses to deliver less than the quote less that slippage.
/// The older forms put those arguments after a variable-length route plan,
/// so they cannot be read without decoding every step, and are refused.
const ROUTE_V2: [u8; 8] = [0xbb, 0x64, 0xfa, 0xcc, 0x31, 0xc4, 0xaf, 0x14];
const SHARED_ACCOUNTS_ROUTE_V2: [u8; 8] = [0xd1, 0x98, 0x53, 0x93, 0x7c, 0xfe, 0xd8, 0xe9];
/// The highest platform fee a route may take, in basis points. The recorded
/// routes carry 10 and 2; the cap leaves five times the higher of them as
/// headroom and stops a route from taking a large share of the output. The
/// simulation's output floor still applies to whatever a route delivers.
pub const MAX_PLATFORM_FEE_BPS: u16 = 50;
/// `route_v2`'s `user_destination_token_account`, which receives the output.
const ROUTE_V2_USER_DESTINATION: usize = 2;
/// `route_v2`'s optional `destination_token_account`, which would receive
/// the output in place of the user's account. Absent, Anchor fills the slot
/// with the program id.
const ROUTE_V2_DESTINATION: usize = 7;
/// `shared_accounts_route_v2`'s `destination_token_account`, which receives
/// the output.
const SHARED_ACCOUNTS_ROUTE_V2_DESTINATION: usize = 5;
/// Compute Budget `SetComputeUnitLimit`, `SetComputeUnitPrice` and
/// `SetLoadedAccountsDataSizeLimit` tags.
const COMPUTE_UNIT_LIMIT_TAG: u8 = 2;
const COMPUTE_UNIT_PRICE_TAG: u8 = 3;
const LOADED_ACCOUNTS_DATA_SIZE_TAG: u8 = 4;
/// `getMultipleAccounts` answers at most this many keys per call.
const MAX_ACCOUNTS_PER_REQUEST: usize = 100;

/// Why a swap order or its transaction was refused. Each variant names the
/// check that failed; none carries a secret or a URL.
#[derive(Debug, thiserror::Error)]
pub enum SwapRefusal {
    /// `actual` is response text, bounded by `bounded_text`.
    #[error("swap refused: order {field} is {actual}, requested {expected}")]
    OrderMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("swap refused: order quotes no output")]
    OutAmountZero,
    #[error("swap refused: order minimum output {threshold} is below the requested minimum {min_output}")]
    ThresholdBelowMinimum { threshold: u64, min_output: u64 },
    #[error("swap refused: order {field} is missing or malformed")]
    OrderMalformed { field: &'static str },
    #[error("swap refused: order minimum output {threshold} is below {min_out}, the least {slippage_bps} bps of slippage allows on {out_amount}")]
    ThresholdBelowSlippage {
        threshold: u64,
        min_out: u64,
        out_amount: u64,
        slippage_bps: u16,
    },
    #[error("swap refused: the transaction's fee payer is not the taker")]
    FeePayerNotTaker,
    #[error("swap refused: lookup table {table} could not be resolved")]
    LookupTableUnresolved { table: Pubkey },
    #[error(
        "swap refused: instruction {index} references an account the transaction does not hold"
    )]
    AccountIndexOutOfRange { index: usize },
    #[error("swap refused: instruction {index} invokes {program}, which a swap does not need")]
    ProgramNotAllowed { index: usize, program: Pubkey },
    #[error("swap refused: instruction {index} to {program} is not a form a swap uses")]
    InstructionNotAllowed { index: usize, program: Pubkey },
    #[error("swap refused: route instruction {index} spends {in_amount}, requested {expected}")]
    RouteInAmount {
        index: usize,
        in_amount: u64,
        expected: u64,
    },
    #[error("swap refused: route instruction {index} accepts {route_minimum}, below the order minimum {min_out}")]
    RouteMinimumBelowOrder {
        index: usize,
        route_minimum: u64,
        min_out: u64,
    },
    #[error("swap refused: route instruction {index} delivers to {account}, not the taker's output account")]
    RouteDestination { index: usize, account: Pubkey },
    #[error(
        "swap refused: route instruction {index} takes a {fee_bps} bps platform fee, above {cap}"
    )]
    PlatformFeeAboveCap {
        index: usize,
        fee_bps: u16,
        cap: u16,
    },
    #[error("swap refused: transaction sets the compute unit limit {count} times, not once")]
    ComputeUnitLimitCount { count: usize },
    #[error("swap refused: compute unit limit {limit} exceeds {cap}")]
    ComputeUnitLimitAboveCap { limit: u32, cap: u32 },
    #[error("swap refused: compute unit price {price} exceeds {cap} micro-lamports")]
    ComputeUnitPriceAboveCap { price: u64, cap: u64 },
    #[error("swap refused: simulation failed: {0}")]
    SimulationFailed(TransactionError),
    #[error("swap refused: simulation did not return every writable account")]
    SimulationIncomplete,
    #[error("swap refused: input spent {actual}, requested exactly {expected}")]
    InputSpent { expected: u64, actual: i128 },
    #[error("swap refused: output received {received}, below the minimum {threshold}")]
    OutputBelowThreshold { threshold: u64, received: i128 },
    #[error("swap refused: taker SOL fell by {spent} lamports, above the bound {bound}")]
    LamportsOverBound { spent: i128, bound: i128 },
    #[error("swap refused: taker balance of {mint} decreased")]
    OtherTokenDecreased { mint: Pubkey },
    #[error("swap refused: the taker's own account changed owner or data")]
    TakerAccountChanged,
    #[error("swap refused: taker token account {account} changed owner")]
    TokenOwnerChanged { account: Pubkey },
    #[error("swap refused: taker token account {account} gained or changed a delegate or close authority")]
    TokenAuthorityChanged { account: Pubkey },
}

/// The swap the caller asked for: what the order and the transaction are
/// checked against.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SwapRequest {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount: u64,
    /// The caller's floor on the output, in the output mint's smallest
    /// units.
    pub min_output: u64,
    pub slippage_bps: u16,
    pub taker: Pubkey,
}

/// A simulation's outcome: its error, if any, and the post-state of the
/// accounts it was asked for, in the order they were asked for.
pub(crate) struct Simulation {
    pub err: Option<TransactionError>,
    pub accounts: Vec<Option<Account>>,
}

/// The chain reads a swap needs, behind a trait so the checks run against
/// recorded state in tests.
pub(crate) trait SwapChain {
    async fn latest_blockhash(&self) -> Result<(Hash, u64), JupiterError>;
    async fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<Account>>, JupiterError>;
    async fn simulate(
        &self,
        txn: &VersionedTransaction,
        keys: &[Pubkey],
    ) -> Result<Simulation, JupiterError>;
}

impl SwapChain for SolanaRpcClient {
    async fn latest_blockhash(&self) -> Result<(Hash, u64), JupiterError> {
        self.get_latest_blockhash_with_commitment(self.commitment())
            .await
            .map_err(JupiterError::solana)
    }

    async fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<Account>>, JupiterError> {
        let mut accounts = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(MAX_ACCOUNTS_PER_REQUEST) {
            let response = self
                .get_multiple_accounts_with_commitment(chunk, self.commitment())
                .await
                .map_err(JupiterError::solana)?;
            accounts.extend(response.value);
        }
        Ok(accounts)
    }

    async fn simulate(
        &self,
        txn: &VersionedTransaction,
        keys: &[Pubkey],
    ) -> Result<Simulation, JupiterError> {
        let config = RpcSimulateTransactionConfig {
            // The transaction is simulated unsigned, so signatures are not
            // verified; replacing the blockhash only spares a node that has
            // not seen it yet from failing the simulation.
            sig_verify: false,
            replace_recent_blockhash: true,
            commitment: Some(self.commitment()),
            encoding: Some(UiTransactionEncoding::Base64),
            accounts: Some(RpcSimulateTransactionAccountsConfig {
                encoding: Some(UiAccountEncoding::Base64),
                addresses: keys.iter().map(ToString::to_string).collect(),
            }),
            min_context_slot: None,
            inner_instructions: false,
        };
        let result = self
            .simulate_transaction_with_config(txn, config)
            .await
            .map_err(JupiterError::solana)?
            .value;
        let accounts = result
            .accounts
            .ok_or(SwapRefusal::SimulationIncomplete)?
            .into_iter()
            .map(|account| match account {
                None => Ok(None),
                Some(ui) => ui
                    .decode::<Account>()
                    .map(Some)
                    .ok_or(SwapRefusal::SimulationIncomplete),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Simulation {
            err: result.err,
            accounts,
        })
    }
}

/// Check `order` against `request`, decode its transaction, check it, set a
/// fresh blockhash, check a simulation of the unsigned transaction, and only
/// then sign it as the taker. Returns the signed transaction, its last valid
/// block height and the order.
pub(crate) async fn sign_verified<S: SwapChain>(
    chain: &S,
    request: &SwapRequest,
    order: OrderResponse,
    keypair: &(dyn Signer + Sync),
) -> Result<(VersionedTransaction, u64, OrderResponse), JupiterError> {
    if order.transaction.is_empty() {
        return Err(JupiterError::SwapError(
            order.error_message.as_deref().map_or_else(
                || "no transaction returned".to_string(),
                super::bounded_text,
            ),
        ));
    }
    let min_out = check_order(&order, request)?;

    let tx_bytes = BASE64
        .decode(&order.transaction)
        .map_err(JupiterError::transaction_decode)?;
    let txn: VersionedTransaction =
        bincode::deserialize(&tx_bytes).map_err(JupiterError::transaction_decode)?;
    super::ensure_taker_only_signer(&txn.message, &request.taker)?;

    let table_keys: Vec<Pubkey> = txn
        .message
        .address_table_lookups()
        .unwrap_or_default()
        .iter()
        .map(|lookup| lookup.account_key)
        .collect();
    let tables = if table_keys.is_empty() {
        Vec::new()
    } else {
        chain.accounts(&table_keys).await?
    };
    let resolved = resolve_accounts(&txn.message, &tables)?;
    let budget = check_instructions(&txn.message, &resolved.keys, request, min_out)?;
    let fee_bound = budget.fee_bound(txn.message.header().num_required_signatures);

    let (blockhash, last_valid_block_height) = chain.latest_blockhash().await?;
    let mut message = txn.message;
    message.set_recent_blockhash(blockhash);
    let unsigned = VersionedTransaction {
        signatures: vec![
            Signature::default();
            usize::from(message.header().num_required_signatures)
        ],
        message,
    };

    let pre = chain.accounts(&resolved.writable).await?;
    let simulation = chain.simulate(&unsigned, &resolved.writable).await?;
    if let Some(err) = simulation.err {
        return Err(SwapRefusal::SimulationFailed(err).into());
    }
    let expected = ExpectedEffects {
        request: *request,
        min_out,
        fee_bound,
    };
    check_effects(&expected, &resolved.writable, &pre, &simulation.accounts)?;

    let txn = VersionedTransaction::try_new(unsigned.message, &[keypair])
        .map_err(JupiterError::signing)?;
    Ok((txn, last_valid_block_height, order))
}

/// Check the order's terms against the request. Returns the order's minimum
/// output (`otherAmountThreshold`), which the route and the simulation must
/// meet; it is at least `request.min_output`, so they meet that too.
pub(crate) fn check_order(
    order: &OrderResponse,
    request: &SwapRequest,
) -> Result<u64, SwapRefusal> {
    fn same(field: &'static str, expected: String, actual: &str) -> Result<(), SwapRefusal> {
        if actual == expected {
            Ok(())
        } else {
            Err(SwapRefusal::OrderMismatch {
                field,
                expected,
                actual: super::bounded_text(actual),
            })
        }
    }
    fn amount(field: &'static str, value: Option<&str>) -> Result<u64, SwapRefusal> {
        value
            .and_then(|value| value.parse().ok())
            .ok_or(SwapRefusal::OrderMalformed { field })
    }

    same(
        "inputMint",
        request.input_mint.to_string(),
        &order.input_mint,
    )?;
    same(
        "outputMint",
        request.output_mint.to_string(),
        &order.output_mint,
    )?;
    same("inAmount", request.amount.to_string(), &order.in_amount)?;
    same(
        "slippageBps",
        request.slippage_bps.to_string(),
        &order.slippage_bps.to_string(),
    )?;
    same(
        "swapMode",
        "ExactIn".to_string(),
        order.swap_mode.as_deref().unwrap_or_default(),
    )?;
    same(
        "taker",
        request.taker.to_string(),
        order.taker.as_deref().unwrap_or_default(),
    )?;

    let out_amount = amount("outAmount", Some(&order.out_amount))?;
    if out_amount == 0 {
        return Err(SwapRefusal::OutAmountZero);
    }
    let threshold = amount(
        "otherAmountThreshold",
        order.other_amount_threshold.as_deref(),
    )?;
    if threshold < request.min_output {
        return Err(SwapRefusal::ThresholdBelowMinimum {
            threshold,
            min_output: request.min_output,
        });
    }
    let min_out = min_out(out_amount, request.slippage_bps);
    if threshold < min_out {
        return Err(SwapRefusal::ThresholdBelowSlippage {
            threshold,
            min_out,
            out_amount,
            slippage_bps: request.slippage_bps,
        });
    }
    Ok(threshold)
}

/// The least output `slippage_bps` allows on a quote of `out_amount`,
/// rounded down as Jupiter rounds it.
fn min_out(out_amount: u64, slippage_bps: u16) -> u64 {
    let kept = 10_000u128.saturating_sub(u128::from(slippage_bps));
    (u128::from(out_amount) * kept / 10_000) as u64
}

/// A message's account keys with its lookup tables resolved, in the order
/// instructions index them, and the subset the transaction may write.
#[derive(Debug)]
pub(crate) struct ResolvedAccounts {
    pub keys: Vec<Pubkey>,
    pub writable: Vec<Pubkey>,
}

/// Resolve `message`'s lookup tables from `tables`, the accounts fetched for
/// them in lookup order. A table that is missing, not owned by the lookup
/// table program, undecodable, or too short for an index is refused: a guess
/// would misalign every index after it.
pub(crate) fn resolve_accounts(
    message: &VersionedMessage,
    tables: &[Option<Account>],
) -> Result<ResolvedAccounts, SwapRefusal> {
    let header = message.header();
    let static_keys = message.static_account_keys();
    let signers = usize::from(header.num_required_signatures);
    let writable_signers = signers.saturating_sub(usize::from(header.num_readonly_signed_accounts));
    let writable_unsigned_end = static_keys
        .len()
        .saturating_sub(usize::from(header.num_readonly_unsigned_accounts));
    let mut writable: Vec<Pubkey> = static_keys
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            *index < writable_signers || (signers..writable_unsigned_end).contains(index)
        })
        .map(|(_, key)| *key)
        .collect();

    let lookups = message.address_table_lookups().unwrap_or_default();
    let mut lookup_writable = Vec::new();
    let mut lookup_readonly = Vec::new();
    for (index, lookup) in lookups.iter().enumerate() {
        let account = tables
            .get(index)
            .and_then(Option::as_ref)
            .filter(|account| account.owner == address_lookup_table::ID)
            .ok_or_else(|| unresolved_table(lookup.account_key))?;
        let table = AddressLookupTable::deserialize(&account.data)
            .map_err(|_| unresolved_table(lookup.account_key))?;
        let entry = |position: &u8| {
            table
                .addresses
                .get(usize::from(*position))
                .copied()
                .ok_or_else(|| unresolved_table(lookup.account_key))
        };
        for position in &lookup.writable_indexes {
            lookup_writable.push(entry(position)?);
        }
        for position in &lookup.readonly_indexes {
            lookup_readonly.push(entry(position)?);
        }
    }

    writable.extend_from_slice(&lookup_writable);
    let keys = static_keys
        .iter()
        .copied()
        .chain(lookup_writable)
        .chain(lookup_readonly)
        .collect();
    Ok(ResolvedAccounts { keys, writable })
}

fn unresolved_table(table: Pubkey) -> SwapRefusal {
    SwapRefusal::LookupTableUnresolved { table }
}

/// The compute budget a transaction requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ComputeBudget {
    pub limit: u32,
    pub price: u64,
}

impl ComputeBudget {
    /// What the fee payer is charged: the base fee for each signature plus
    /// the priority fee at the requested limit.
    pub fn fee_bound(&self, signatures: u8) -> u64 {
        let limit = u128::from(self.limit);
        let priority = (u128::from(self.price) * limit).div_ceil(MICRO_LAMPORTS_PER_LAMPORT);
        LAMPORTS_PER_SIGNATURE * u64::from(signatures) + priority as u64
    }
}

/// Refuse any top-level instruction that is not one of the forms a Jupiter
/// aggregator route emits. Returns the compute budget the message requests.
///
/// Allowed:
/// - Compute Budget: exactly one `SetComputeUnitLimit`, up to
///   [`MAX_COMPUTE_UNIT_LIMIT`], so the fee bound is the fee charged;
///   `SetComputeUnitPrice` up to [`MAX_COMPUTE_UNIT_PRICE`]; and
///   `SetLoadedAccountsDataSizeLimit`, which only restricts the transaction.
/// - Associated Token Account: `CreateIdempotent`, paid for by the taker,
///   for an account the taker owns, under SPL Token or Token-2022.
/// - System: `Transfer`s of exactly `amount` lamports each, from the taker
///   to the taker's wrapped-SOL account, and only when the input is SOL.
/// - SPL Token: `SyncNative` of the taker's wrapped-SOL account, and
///   `CloseAccount` of it back to the taker, by the taker.
/// - Jupiter aggregator `route_v2` and `shared_accounts_route_v2` spending
///   exactly `amount`, whose own minimum output is at least `min_out`, whose
///   platform fee is at most [`MAX_PLATFORM_FEE_BPS`], and that deliver to
///   the taker's associated token account of the output mint (`route_v2`'s
///   optional destination left empty). What the route does through other
///   programs is checked by simulation.
pub(crate) fn check_instructions(
    message: &VersionedMessage,
    keys: &[Pubkey],
    request: &SwapRequest,
    min_out: u64,
) -> Result<ComputeBudget, SwapRefusal> {
    let taker = request.taker;
    if message.static_account_keys().first() != Some(&taker) {
        return Err(SwapRefusal::FeePayerNotTaker);
    }
    let wrapped_sol = get_associated_token_address(&taker, &native_mint::ID);
    let output_accounts = [
        get_associated_token_address(&taker, &request.output_mint),
        get_associated_token_address_with_program_id(&taker, &request.output_mint, &TOKEN_2022),
    ];
    let mut limits = Vec::new();
    let mut price = 0;

    for (index, ix) in message.instructions().iter().enumerate() {
        let key = |position: u8| {
            keys.get(usize::from(position))
                .copied()
                .ok_or(SwapRefusal::AccountIndexOutOfRange { index })
        };
        let account = |n: usize| -> Result<Pubkey, SwapRefusal> {
            key(*ix
                .accounts
                .get(n)
                .ok_or(SwapRefusal::AccountIndexOutOfRange { index })?)
        };
        let program = key(ix.program_id_index)?;
        let not_allowed = SwapRefusal::InstructionNotAllowed { index, program };
        let data = ix.data.as_slice();

        if program == compute_budget::ID {
            match data.split_first() {
                Some((&COMPUTE_UNIT_LIMIT_TAG, value)) => {
                    let limit = u32::from_le_bytes(value.try_into().map_err(|_| not_allowed)?);
                    if limit > MAX_COMPUTE_UNIT_LIMIT {
                        return Err(SwapRefusal::ComputeUnitLimitAboveCap {
                            limit,
                            cap: MAX_COMPUTE_UNIT_LIMIT,
                        });
                    }
                    limits.push(limit);
                }
                Some((&COMPUTE_UNIT_PRICE_TAG, value)) => {
                    let value_price =
                        u64::from_le_bytes(value.try_into().map_err(|_| not_allowed)?);
                    if value_price > MAX_COMPUTE_UNIT_PRICE {
                        return Err(SwapRefusal::ComputeUnitPriceAboveCap {
                            price: value_price,
                            cap: MAX_COMPUTE_UNIT_PRICE,
                        });
                    }
                    price = value_price;
                }
                Some((&LOADED_ACCOUNTS_DATA_SIZE_TAG, value)) if value.len() == 4 => {}
                _ => return Err(not_allowed),
            }
        } else if program == spl_associated_token_account::ID {
            // Accounts: payer, associated account, wallet, mint, system
            // program, token program. The program itself derives the
            // associated account from the rest.
            let token_program = account(5)?;
            let allowed = data == ATA_CREATE_IDEMPOTENT
                && account(0)? == taker
                && account(2)? == taker
                && (token_program == spl_token::ID || token_program == TOKEN_2022);
            if !allowed {
                return Err(not_allowed);
            }
        } else if program == system_program::ID {
            let mut transfer = SYSTEM_TRANSFER_TAG.to_vec();
            transfer.extend_from_slice(&request.amount.to_le_bytes());
            let allowed = request.input_mint == native_mint::ID
                && data == transfer.as_slice()
                && account(0)? == taker
                && account(1)? == wrapped_sol;
            if !allowed {
                return Err(not_allowed);
            }
        } else if program == spl_token::ID {
            let allowed = if data == TOKEN_SYNC_NATIVE {
                account(0)? == wrapped_sol
            } else if data == TOKEN_CLOSE_ACCOUNT {
                account(0)? == wrapped_sol && account(1)? == taker && account(2)? == taker
            } else {
                false
            };
            if !allowed {
                return Err(not_allowed);
            }
        } else if program == JUPITER_AGGREGATOR_V6 {
            let (terms, destination) = match data.split_at_checked(ROUTE_V2.len()) {
                Some((discriminator, args)) if discriminator == ROUTE_V2 => {
                    let placeholder = account(ROUTE_V2_DESTINATION)?;
                    if placeholder != JUPITER_AGGREGATOR_V6 {
                        return Err(SwapRefusal::RouteDestination {
                            index,
                            account: placeholder,
                        });
                    }
                    (Some(args), account(ROUTE_V2_USER_DESTINATION)?)
                }
                Some((discriminator, args)) if discriminator == SHARED_ACCOUNTS_ROUTE_V2 => (
                    args.get(1..),
                    account(SHARED_ACCOUNTS_ROUTE_V2_DESTINATION)?,
                ),
                _ => return Err(not_allowed),
            };
            let terms = terms.and_then(RouteTerms::read).ok_or(not_allowed)?;
            if !output_accounts.contains(&destination) {
                return Err(SwapRefusal::RouteDestination {
                    index,
                    account: destination,
                });
            }
            if terms.platform_fee_bps > MAX_PLATFORM_FEE_BPS {
                return Err(SwapRefusal::PlatformFeeAboveCap {
                    index,
                    fee_bps: terms.platform_fee_bps,
                    cap: MAX_PLATFORM_FEE_BPS,
                });
            }
            if terms.in_amount != request.amount {
                return Err(SwapRefusal::RouteInAmount {
                    index,
                    in_amount: terms.in_amount,
                    expected: request.amount,
                });
            }
            let route_minimum = self::min_out(terms.quoted_out_amount, terms.slippage_bps);
            if route_minimum < min_out {
                return Err(SwapRefusal::RouteMinimumBelowOrder {
                    index,
                    route_minimum,
                    min_out,
                });
            }
        } else {
            return Err(SwapRefusal::ProgramNotAllowed { index, program });
        }
    }
    match limits.as_slice() {
        [limit] => Ok(ComputeBudget {
            limit: *limit,
            price,
        }),
        _ => Err(SwapRefusal::ComputeUnitLimitCount {
            count: limits.len(),
        }),
    }
}

/// The leading arguments of a Jupiter v2 route.
struct RouteTerms {
    in_amount: u64,
    quoted_out_amount: u64,
    slippage_bps: u16,
    platform_fee_bps: u16,
}

impl RouteTerms {
    fn read(args: &[u8]) -> Option<Self> {
        Some(Self {
            in_amount: u64::from_le_bytes(args.get(0..8)?.try_into().ok()?),
            quoted_out_amount: u64::from_le_bytes(args.get(8..16)?.try_into().ok()?),
            slippage_bps: u16::from_le_bytes(args.get(16..18)?.try_into().ok()?),
            platform_fee_bps: u16::from_le_bytes(args.get(18..20)?.try_into().ok()?),
        })
    }
}

/// What the simulation must show.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExpectedEffects {
    pub request: SwapRequest,
    /// The least output the taker must receive.
    pub min_out: u64,
    /// The most SOL the transaction's fees can cost the taker.
    pub fee_bound: u64,
}

/// What the taker holds among a set of accounts: SOL in lamports (the
/// taker's own plus every lamport in its token accounts, so wrapped SOL and
/// token account rent count as the taker's), and every other mint in base
/// units.
#[derive(Debug, Default)]
struct Holdings {
    lamports: i128,
    tokens: HashMap<Pubkey, i128>,
}

impl Holdings {
    fn of<'a>(taker: &Pubkey, accounts: impl Iterator<Item = (&'a Pubkey, &'a Account)>) -> Self {
        let mut holdings = Self::default();
        for (address, account) in accounts {
            if address == taker {
                holdings.lamports += i128::from(account.lamports);
                continue;
            }
            let Some(token) = token_account(account).filter(|token| token.owner == *taker) else {
                continue;
            };
            holdings.lamports += i128::from(account.lamports);
            if token.mint != native_mint::ID {
                *holdings.tokens.entry(token.mint).or_default() += i128::from(token.amount);
            }
        }
        holdings
    }

    fn token(&self, mint: &Pubkey) -> i128 {
        self.tokens.get(mint).copied().unwrap_or_default()
    }
}

/// Decode an SPL Token or Token-2022 token account; `None` for anything
/// else (a mint, a multisig, a closed or system account).
fn token_account(account: &Account) -> Option<TokenAccount> {
    if account.owner != spl_token::ID && account.owner != TOKEN_2022 {
        return None;
    }
    let data = account.data.as_slice();
    let is_token_account = data.len() == TokenAccount::LEN
        || (data.len() > TokenAccount::LEN
            && data.len() != Multisig::LEN
            && data[TokenAccount::LEN] == TOKEN_2022_ACCOUNT_TYPE);
    if !is_token_account {
        return None;
    }
    TokenAccount::unpack(&data[..TokenAccount::LEN]).ok()
}

/// Compare the writable accounts before (`pre`) and after (`post`) the
/// simulated swap against what `expected` allows.
pub(crate) fn check_effects(
    expected: &ExpectedEffects,
    writable: &[Pubkey],
    pre: &[Option<Account>],
    post: &[Option<Account>],
) -> Result<(), SwapRefusal> {
    if pre.len() != writable.len() || post.len() != writable.len() {
        return Err(SwapRefusal::SimulationIncomplete);
    }
    let request = &expected.request;
    let taker = &request.taker;

    for ((address, before), after) in writable.iter().zip(pre).zip(post) {
        if address == taker {
            let unchanged = match (before, after) {
                (Some(before), Some(after)) => {
                    before.owner == after.owner && before.data == after.data
                }
                _ => false,
            };
            if !unchanged {
                return Err(SwapRefusal::TakerAccountChanged);
            }
            continue;
        }
        let before = before
            .as_ref()
            .and_then(token_account)
            .filter(|token| token.owner == *taker);
        let after = after.as_ref().and_then(token_account);
        match (before, after) {
            (Some(before), Some(after)) => {
                if after.owner != *taker {
                    return Err(SwapRefusal::TokenOwnerChanged { account: *address });
                }
                if after.delegate != before.delegate
                    || after.close_authority != before.close_authority
                {
                    return Err(SwapRefusal::TokenAuthorityChanged { account: *address });
                }
            }
            (None, Some(after))
                if after.owner == *taker
                    && (after.delegate.is_some() || after.close_authority.is_some()) =>
            {
                return Err(SwapRefusal::TokenAuthorityChanged { account: *address });
            }
            _ => {}
        }
    }

    let held = |accounts: &'_ [Option<Account>]| {
        Holdings::of(
            taker,
            writable
                .iter()
                .zip(accounts)
                .filter_map(|(address, account)| {
                    account.as_ref().map(|account| (address, account))
                }),
        )
    };
    let before = held(pre);
    let after = held(post);
    let token_delta = |mint: &Pubkey| after.token(mint) - before.token(mint);

    let input_is_sol = request.input_mint == native_mint::ID;
    let output_is_sol = request.output_mint == native_mint::ID;
    if !input_is_sol {
        let spent = -token_delta(&request.input_mint);
        if spent != i128::from(request.amount) {
            return Err(SwapRefusal::InputSpent {
                expected: request.amount,
                actual: spent,
            });
        }
    }
    if !output_is_sol {
        let received = token_delta(&request.output_mint);
        if received < i128::from(expected.min_out) {
            return Err(SwapRefusal::OutputBelowThreshold {
                threshold: expected.min_out,
                received,
            });
        }
    }
    let other_mints: BTreeSet<&Pubkey> = before
        .tokens
        .keys()
        .chain(after.tokens.keys())
        .filter(|mint| **mint != request.input_mint && **mint != request.output_mint)
        .collect();
    if let Some(mint) = other_mints.into_iter().find(|mint| token_delta(mint) < 0) {
        return Err(SwapRefusal::OtherTokenDecreased { mint: *mint });
    }

    let lamport_delta = after.lamports - before.lamports;
    let fee_bound = i128::from(expected.fee_bound);
    if output_is_sol {
        // The output arrives as lamports, which also pay the fee.
        let received = lamport_delta + fee_bound;
        if received < i128::from(expected.min_out) {
            return Err(SwapRefusal::OutputBelowThreshold {
                threshold: expected.min_out,
                received,
            });
        }
    } else {
        let bound = fee_bound
            + if input_is_sol {
                i128::from(request.amount)
            } else {
                0
            };
        if -lamport_delta > bound {
            return Err(SwapRefusal::LamportsOverBound {
                spent: -lamport_delta,
                bound,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solana_sdk::{
        address_lookup_table::AddressLookupTableAccount,
        instruction::{AccountMeta, Instruction},
        message::{v0, Message},
        program_option::COption,
        signature::NullSigner,
        signer::SignerError,
    };
    use serde::Deserialize;
    use solana_address_lookup_table_interface::state::LookupTableMeta;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{borrow::Cow, collections::BTreeMap, str::FromStr};

    const HNT: Pubkey = pubkey!("hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux");
    const USDC: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    const MOBILE: Pubkey = pubkey!("mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6");

    // Orders recorded from the v2 `/order` endpoint (router `metis`) for
    // two public wallets, with the lookup-table entries their transactions
    // reference and the taker's accounts before and after a mainnet
    // simulation of each transaction.
    const HNT_USDC: &str = include_str!("../../fixtures/jupiter/hnt_usdc.json");
    const SOL_USDC: &str = include_str!("../../fixtures/jupiter/sol_usdc.json");

    #[derive(Deserialize)]
    struct Fixture {
        order: OrderResponse,
        lookup_tables: HashMap<String, BTreeMap<String, String>>,
        accounts: Vec<FixtureAccount>,
    }

    #[derive(Deserialize)]
    struct FixtureAccount {
        address: String,
        pre: Option<FixtureState>,
        post: Option<FixtureState>,
    }

    #[derive(Deserialize)]
    struct FixtureState {
        lamports: u64,
        owner: String,
        data: String,
    }

    fn key(value: &str) -> Pubkey {
        Pubkey::from_str(value).expect("fixture pubkey")
    }

    impl FixtureState {
        fn account(&self) -> Account {
            Account {
                lamports: self.lamports,
                data: BASE64.decode(&self.data).expect("fixture account data"),
                owner: key(&self.owner),
                executable: false,
                rent_epoch: u64::MAX,
            }
        }
    }

    fn lookup_table(entries: &BTreeMap<String, String>) -> Account {
        let len = entries
            .keys()
            .map(|index| index.parse::<usize>().expect("fixture table index") + 1)
            .max()
            .unwrap_or_default();
        let mut addresses = vec![Pubkey::default(); len];
        for (index, address) in entries {
            addresses[index.parse::<usize>().expect("fixture table index")] = key(address);
        }
        let data = AddressLookupTable {
            meta: LookupTableMeta::default(),
            addresses: Cow::Owned(addresses),
        }
        .serialize_for_tests()
        .expect("serialize lookup table");
        Account {
            lamports: 1,
            data,
            owner: address_lookup_table::ID,
            executable: false,
            rent_epoch: u64::MAX,
        }
    }

    /// Recorded chain state: `pre` answers account reads (lookup tables
    /// included), `post` answers the simulation.
    struct FakeChain {
        pre: HashMap<Pubkey, Account>,
        post: HashMap<Pubkey, Account>,
        sim_err: Option<TransactionError>,
        drop_post: bool,
    }

    const BLOCKHASH: Hash = Hash::new_from_array([7; 32]);
    const LAST_VALID_BLOCK_HEIGHT: u64 = 42;

    impl SwapChain for FakeChain {
        async fn latest_blockhash(&self) -> Result<(Hash, u64), JupiterError> {
            Ok((BLOCKHASH, LAST_VALID_BLOCK_HEIGHT))
        }

        async fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<Account>>, JupiterError> {
            Ok(keys.iter().map(|key| self.pre.get(key).cloned()).collect())
        }

        async fn simulate(
            &self,
            txn: &VersionedTransaction,
            keys: &[Pubkey],
        ) -> Result<Simulation, JupiterError> {
            assert_eq!(*txn.message.recent_blockhash(), BLOCKHASH);
            assert!(
                txn.signatures
                    .iter()
                    .all(|signature| *signature == Signature::default()),
                "the simulated transaction carries a signature"
            );
            let mut accounts: Vec<_> = keys.iter().map(|key| self.post.get(key).cloned()).collect();
            if self.drop_post {
                accounts.pop();
            }
            Ok(Simulation {
                err: self.sim_err.clone(),
                accounts,
            })
        }
    }

    struct Case {
        order: OrderResponse,
        request: SwapRequest,
        chain: FakeChain,
    }

    fn case(fixture: &str) -> Case {
        let fixture: Fixture = serde_json::from_str(fixture).expect("parse fixture");
        let mut pre = HashMap::new();
        let mut post = HashMap::new();
        for (table, entries) in &fixture.lookup_tables {
            pre.insert(key(table), lookup_table(entries));
        }
        for account in &fixture.accounts {
            let address = key(&account.address);
            if let Some(state) = &account.pre {
                pre.insert(address, state.account());
            }
            if let Some(state) = &account.post {
                post.insert(address, state.account());
            }
        }
        let order = fixture.order;
        let threshold: u64 = order
            .other_amount_threshold
            .as_deref()
            .expect("fixture otherAmountThreshold")
            .parse()
            .expect("fixture otherAmountThreshold");
        let request = SwapRequest {
            input_mint: key(&order.input_mint),
            output_mint: key(&order.output_mint),
            amount: order.in_amount.parse().expect("fixture inAmount"),
            // Just below the order's own minimum, which must still pass.
            min_output: threshold - 1,
            slippage_bps: order.slippage_bps,
            taker: key(order.taker.as_deref().expect("fixture taker")),
        };
        Case {
            order,
            request,
            chain: FakeChain {
                pre,
                post,
                sim_err: None,
                drop_post: false,
            },
        }
    }

    /// A signer that counts the messages it signs.
    struct CountingSigner {
        inner: NullSigner,
        signed: AtomicUsize,
    }

    impl CountingSigner {
        fn new(pubkey: &Pubkey) -> Self {
            Self {
                inner: NullSigner::new(pubkey),
                signed: AtomicUsize::new(0),
            }
        }

        fn signed(&self) -> usize {
            self.signed.load(Ordering::SeqCst)
        }
    }

    impl Signer for CountingSigner {
        fn try_pubkey(&self) -> Result<Pubkey, SignerError> {
            self.inner.try_pubkey()
        }

        fn try_sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
            self.signed.fetch_add(1, Ordering::SeqCst);
            self.inner.try_sign_message(message)
        }

        fn is_interactive(&self) -> bool {
            false
        }
    }

    impl Case {
        async fn run(&self) -> Result<(VersionedTransaction, u64, OrderResponse), JupiterError> {
            let signer = NullSigner::new(&self.request.taker);
            sign_verified(&self.chain, &self.request, self.order.clone(), &signer).await
        }

        /// Run with a counting signer and return the result with the number
        /// of messages signed.
        async fn run_counting(
            &self,
        ) -> (
            Result<(VersionedTransaction, u64, OrderResponse), JupiterError>,
            usize,
        ) {
            let signer = CountingSigner::new(&self.request.taker);
            let result =
                sign_verified(&self.chain, &self.request, self.order.clone(), &signer).await;
            (result, signer.signed())
        }

        async fn refusal(&self) -> SwapRefusal {
            match self.run().await {
                Err(JupiterError::Refused(refusal)) => refusal,
                other => panic!("expected a refusal, got {other:?}"),
            }
        }

        fn message(&self) -> VersionedMessage {
            let bytes = BASE64
                .decode(&self.order.transaction)
                .expect("decode fixture");
            bincode::deserialize::<VersionedTransaction>(&bytes)
                .expect("deserialize fixture")
                .message
        }

        fn set_message(&mut self, message: VersionedMessage) {
            let txn = VersionedTransaction {
                signatures: vec![Default::default()],
                message,
            };
            self.order.transaction = BASE64.encode(bincode::serialize(&txn).expect("serialize"));
        }

        fn resolved(&self) -> ResolvedAccounts {
            let message = self.message();
            let tables: Vec<_> = message
                .address_table_lookups()
                .unwrap_or_default()
                .iter()
                .map(|lookup| self.chain.pre.get(&lookup.account_key).cloned())
                .collect();
            resolve_accounts(&message, &tables).expect("resolve fixture")
        }

        /// The taker's token account of `mint` in the recorded state.
        fn token_account_of(&self, mint: &Pubkey) -> Pubkey {
            *self
                .chain
                .pre
                .iter()
                .find(|(_, account)| {
                    token_account(account).is_some_and(|token| {
                        token.mint == *mint && token.owner == self.request.taker
                    })
                })
                .expect("fixture holds a taker token account of the mint")
                .0
        }

        fn edit_post_token(&mut self, address: &Pubkey, edit: impl FnOnce(&mut TokenAccount)) {
            let account = self.chain.post.get_mut(address).expect("post account");
            edit_token(account, edit);
        }

        fn token_amount(&self, state: &HashMap<Pubkey, Account>, address: &Pubkey) -> u64 {
            token_account(state.get(address).expect("account"))
                .expect("token account")
                .amount
        }

        fn fee_bound(&self) -> u64 {
            let message = self.message();
            let min_out = check_order(&self.order, &self.request).expect("fixture order passes");
            check_instructions(&message, &self.resolved().keys, &self.request, min_out)
                .expect("fixture instructions pass")
                .fee_bound(message.header().num_required_signatures)
        }
    }

    fn edit_token(account: &mut Account, edit: impl FnOnce(&mut TokenAccount)) {
        let mut token = TokenAccount::unpack(&account.data[..TokenAccount::LEN]).expect("unpack");
        edit(&mut token);
        token.pack_into_slice(&mut account.data[..TokenAccount::LEN]);
    }

    fn token_account_state(mint: Pubkey, owner: Pubkey, amount: u64) -> Account {
        let token = TokenAccount {
            mint,
            owner,
            amount,
            state: spl_token::state::AccountState::Initialized,
            ..TokenAccount::default()
        };
        let mut data = vec![0; TokenAccount::LEN];
        token.pack_into_slice(&mut data);
        Account {
            lamports: 2_039_280,
            data,
            owner: spl_token::ID,
            executable: false,
            rent_epoch: u64::MAX,
        }
    }

    // ---- recorded transactions ----

    #[tokio::test]
    async fn recorded_hnt_to_usdc_swap_is_signed() {
        let case = case(HNT_USDC);
        let (result, signed) = case.run_counting().await;
        let (txn, last_valid_block_height, order) = result.expect("signed");
        assert_eq!(signed, 1);
        assert_eq!(*txn.message.recent_blockhash(), BLOCKHASH);
        assert_eq!(last_valid_block_height, LAST_VALID_BLOCK_HEIGHT);
        assert_eq!(txn.signatures.len(), 1);
        assert_eq!(order.in_amount, "100000000");
    }

    #[tokio::test]
    async fn recorded_sol_to_usdc_swap_is_signed() {
        let case = case(SOL_USDC);
        case.run().await.expect("signed");
    }

    #[test]
    fn recorded_transactions_spend_exactly_their_fee_bound() {
        // The simulations charged exactly the fee each message declares,
        // so the recorded swaps sit on the lamport bound.
        let hnt = case(HNT_USDC);
        assert_eq!(hnt.fee_bound(), 5_000 + 13_154);
        let sol = case(SOL_USDC);
        assert_eq!(sol.fee_bound(), 5_000 + 54_926);
    }

    // ---- order ----

    #[tokio::test]
    async fn order_for_another_amount_is_refused() {
        let mut case = case(HNT_USDC);
        case.request.amount += 1;
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::OrderMismatch {
                field: "inAmount",
                ..
            }
        ));
    }

    fn order_field(edit: impl FnOnce(&mut OrderResponse, &mut SwapRequest)) -> SwapRefusal {
        let Case {
            mut order,
            mut request,
            ..
        } = case(HNT_USDC);
        assert!(check_order(&order, &request).is_ok());
        edit(&mut order, &mut request);
        check_order(&order, &request).expect_err("refused")
    }

    #[test]
    fn order_terms_must_match_the_request() {
        let mismatch = |refusal: SwapRefusal| match refusal {
            SwapRefusal::OrderMismatch { field, .. } => field,
            other => panic!("expected OrderMismatch, got {other:?}"),
        };
        assert_eq!(
            mismatch(order_field(|order, _| order.input_mint = MOBILE.to_string())),
            "inputMint"
        );
        assert_eq!(
            mismatch(order_field(
                |order, _| order.output_mint = MOBILE.to_string()
            )),
            "outputMint"
        );
        assert_eq!(
            mismatch(order_field(
                |order, _| order.in_amount = "99999999".to_string()
            )),
            "inAmount"
        );
        assert_eq!(
            mismatch(order_field(|_, request| request.slippage_bps = 50)),
            "slippageBps"
        );
        assert_eq!(
            mismatch(order_field(
                |order, _| order.swap_mode = Some("ExactOut".to_string())
            )),
            "swapMode"
        );
        assert_eq!(
            mismatch(order_field(|order, _| order.taker = None)),
            "taker"
        );
        assert_eq!(
            mismatch(order_field(
                |_, request| request.taker = Pubkey::new_unique()
            )),
            "taker"
        );
    }

    #[test]
    fn order_threshold_must_honour_the_slippage() {
        // 353926 at 100 bps rounds down to 350386.
        let Case {
            mut order, request, ..
        } = case(HNT_USDC);
        assert_eq!(order.other_amount_threshold.as_deref(), Some("350386"));
        assert_eq!(
            check_order(&order, &request).expect("at the floor"),
            350_386
        );
        order.other_amount_threshold = Some("350385".to_string());
        assert!(matches!(
            check_order(&order, &request),
            Err(SwapRefusal::ThresholdBelowSlippage {
                min_out: 350_386,
                ..
            })
        ));
        order.other_amount_threshold = None;
        assert!(matches!(
            check_order(&order, &request),
            Err(SwapRefusal::OrderMalformed {
                field: "otherAmountThreshold"
            })
        ));
        assert!(matches!(
            order_field(|order, _| order.out_amount = "lots".to_string()),
            SwapRefusal::OrderMalformed { field: "outAmount" }
        ));
    }

    // ---- lookup tables ----

    #[tokio::test]
    async fn missing_lookup_table_is_refused() {
        let mut case = case(HNT_USDC);
        let table = case.message().address_table_lookups().expect("v0")[0].account_key;
        case.chain.pre.remove(&table);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::LookupTableUnresolved { table: refused } if refused == table
        ));
    }

    #[tokio::test]
    async fn lookup_table_not_owned_by_the_lookup_table_program_is_refused() {
        let mut case = case(HNT_USDC);
        let table = case.message().address_table_lookups().expect("v0")[0].account_key;
        case.chain.pre.get_mut(&table).expect("table").owner = Pubkey::new_unique();
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::LookupTableUnresolved { .. }
        ));
    }

    #[tokio::test]
    async fn lookup_table_too_short_for_an_index_is_refused() {
        let mut case = case(HNT_USDC);
        let lookup = case.message().address_table_lookups().expect("v0")[0].clone();
        let highest = lookup
            .writable_indexes
            .iter()
            .chain(&lookup.readonly_indexes)
            .max()
            .copied()
            .expect("indexes");
        let account = case.chain.pre.get_mut(&lookup.account_key).expect("table");
        let table = AddressLookupTable::deserialize(&account.data).expect("table");
        let addresses = table.addresses[..usize::from(highest)].to_vec();
        account.data = AddressLookupTable {
            meta: LookupTableMeta::default(),
            addresses: Cow::Owned(addresses),
        }
        .serialize_for_tests()
        .expect("serialize");
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::LookupTableUnresolved { .. }
        ));
    }

    // ---- instructions ----

    #[tokio::test]
    async fn recorded_transaction_with_its_price_raised_past_the_cap_is_refused() {
        let mut case = case(HNT_USDC);
        let mut message = case.message();
        let VersionedMessage::V0(v0) = &mut message else {
            panic!("fixture is v0");
        };
        let price = v0
            .instructions
            .iter_mut()
            .find(|ix| ix.data.first() == Some(&COMPUTE_UNIT_PRICE_TAG))
            .expect("price instruction");
        price.data[1..].copy_from_slice(&(MAX_COMPUTE_UNIT_PRICE + 1).to_le_bytes());
        case.set_message(message);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::ComputeUnitPriceAboveCap { .. }
        ));
    }

    fn sol_request() -> SwapRequest {
        SwapRequest {
            input_mint: native_mint::ID,
            output_mint: USDC,
            amount: 10_000_000,
            min_output: ORDER_MIN_OUT,
            slippage_bps: 100,
            taker: Pubkey::new_unique(),
        }
    }

    fn hnt_request() -> SwapRequest {
        SwapRequest {
            input_mint: HNT,
            ..sol_request()
        }
    }

    fn price(micro_lamports: u64) -> Instruction {
        compute_budget::ComputeBudgetInstruction::set_compute_unit_price(micro_lamports)
    }

    fn limit(units: u32) -> Instruction {
        compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(units)
    }

    fn create_ata(payer: &Pubkey, wallet: &Pubkey, mint: &Pubkey) -> Instruction {
        spl_associated_token_account::instruction::create_associated_token_account_idempotent(
            payer,
            wallet,
            mint,
            &spl_token::ID,
        )
    }

    /// The quote the synthetic routes carry, and the order minimum 100 bps
    /// of slippage leaves of it.
    const QUOTED_OUT: u64 = 1_000_000;
    const ORDER_MIN_OUT: u64 = 990_000;

    /// `route_v2` data: discriminator, `in_amount`, `quoted_out_amount`,
    /// `slippage_bps`, `platform_fee_bps`, `positive_slippage_bps`, and an
    /// empty route plan.
    fn route_data(in_amount: u64, quoted_out: u64, slippage_bps: u16) -> Vec<u8> {
        let mut data = ROUTE_V2.to_vec();
        data.extend_from_slice(&in_amount.to_le_bytes());
        data.extend_from_slice(&quoted_out.to_le_bytes());
        data.extend_from_slice(&slippage_bps.to_le_bytes());
        data.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        data
    }

    /// `data` with its platform fee set to `fee_bps`.
    fn with_platform_fee(mut data: Vec<u8>, fee_bps: u16) -> Vec<u8> {
        data[26..28].copy_from_slice(&fee_bps.to_le_bytes());
        data
    }

    /// A `route_v2` instruction in the recorded account layout: authority,
    /// source, user destination, source and destination mints, their token
    /// programs, the optional destination (empty), event authority, program.
    fn route(taker: &Pubkey, data: Vec<u8>) -> Instruction {
        Instruction::new_with_bytes(
            JUPITER_AGGREGATOR_V6,
            &data,
            vec![
                AccountMeta::new(*taker, true),
                AccountMeta::new(get_associated_token_address(taker, &HNT), false),
                AccountMeta::new(get_associated_token_address(taker, &USDC), false),
                AccountMeta::new_readonly(HNT, false),
                AccountMeta::new_readonly(USDC, false),
                AccountMeta::new_readonly(spl_token::ID, false),
                AccountMeta::new_readonly(spl_token::ID, false),
                AccountMeta::new_readonly(JUPITER_AGGREGATOR_V6, false),
                AccountMeta::new_readonly(Pubkey::new_unique(), false),
                AccountMeta::new_readonly(JUPITER_AGGREGATOR_V6, false),
            ],
        )
    }

    fn wrapped_sol(taker: &Pubkey) -> Pubkey {
        get_associated_token_address(taker, &native_mint::ID)
    }

    fn wrap(request: &SwapRequest, lamports: u64, to: &Pubkey) -> Instruction {
        solana_system_interface::instruction::transfer(&request.taker, to, lamports)
    }

    /// The top-level shape of the recorded SOL-input swap: budget, wrap the
    /// input, create the output account, route, unwrap.
    fn swap_instructions(request: &SwapRequest) -> Vec<Instruction> {
        let taker = &request.taker;
        let wsol = wrapped_sol(taker);
        vec![
            limit(274_672),
            price(199_967),
            create_ata(taker, taker, &native_mint::ID),
            wrap(request, request.amount, &wsol),
            spl_token::instruction::sync_native(&spl_token::ID, &wsol).expect("sync"),
            create_ata(taker, taker, &USDC),
            route(taker, route_data(request.amount, QUOTED_OUT, 100)),
            spl_token::instruction::close_account(&spl_token::ID, &wsol, taker, taker, &[])
                .expect("close"),
        ]
    }

    fn check(
        request: &SwapRequest,
        instructions: &[Instruction],
    ) -> Result<ComputeBudget, SwapRefusal> {
        let message = VersionedMessage::Legacy(Message::new(instructions, Some(&request.taker)));
        let keys = message.static_account_keys().to_vec();
        check_instructions(&message, &keys, request, ORDER_MIN_OUT)
    }

    /// The baseline passes, then `extra` appended to it is refused.
    fn refused_with(request: &SwapRequest, extra: Instruction) -> SwapRefusal {
        let mut instructions = swap_instructions(request);
        check(request, &instructions).expect("baseline passes");
        instructions.push(extra);
        check(request, &instructions).expect_err("refused")
    }

    fn assert_not_allowed(refusal: SwapRefusal, expected_program: Pubkey) {
        match refusal {
            SwapRefusal::InstructionNotAllowed { program, .. } => {
                assert_eq!(program, expected_program)
            }
            other => panic!("expected InstructionNotAllowed, got {other:?}"),
        }
    }

    #[test]
    fn swap_shape_passes_and_reports_its_budget() {
        let request = sol_request();
        assert_eq!(
            check(&request, &swap_instructions(&request)).expect("passes"),
            ComputeBudget {
                limit: 274_672,
                price: 199_967,
            }
        );
    }

    #[test]
    fn token_transfer_out_of_the_takers_account_is_refused() {
        let request = sol_request();
        let taker = request.taker;
        let transfer = spl_token::instruction::transfer(
            &spl_token::ID,
            &get_associated_token_address(&taker, &USDC),
            &get_associated_token_address(&Pubkey::new_unique(), &USDC),
            &taker,
            &[],
            1,
        )
        .expect("transfer");
        assert_not_allowed(refused_with(&request, transfer), spl_token::ID);
    }

    #[test]
    fn set_authority_on_the_takers_account_is_refused() {
        let request = sol_request();
        let taker = request.taker;
        let set_authority = spl_token::instruction::set_authority(
            &spl_token::ID,
            &get_associated_token_address(&taker, &USDC),
            Some(&Pubkey::new_unique()),
            spl_token::instruction::AuthorityType::AccountOwner,
            &taker,
            &[],
        )
        .expect("set_authority");
        assert_not_allowed(refused_with(&request, set_authority), spl_token::ID);
    }

    #[test]
    fn approve_on_the_takers_account_is_refused() {
        let request = sol_request();
        let taker = request.taker;
        let approve = spl_token::instruction::approve(
            &spl_token::ID,
            &get_associated_token_address(&taker, &USDC),
            &Pubkey::new_unique(),
            &taker,
            &[],
            u64::MAX,
        )
        .expect("approve");
        assert_not_allowed(refused_with(&request, approve), spl_token::ID);
    }

    #[test]
    fn system_transfer_to_another_account_is_refused() {
        let request = sol_request();
        let transfer = wrap(&request, request.amount, &Pubkey::new_unique());
        assert_not_allowed(refused_with(&request, transfer), system_program::ID);
    }

    #[test]
    fn wrapping_more_than_the_amount_is_refused() {
        let request = sol_request();
        let transfer = wrap(&request, request.amount + 1, &wrapped_sol(&request.taker));
        assert_not_allowed(refused_with(&request, transfer), system_program::ID);
    }

    #[test]
    fn wrapping_sol_for_a_token_input_is_refused() {
        let request = hnt_request();
        let mut instructions = swap_instructions(&request);
        instructions.remove(3);
        check(&request, &instructions).expect("baseline without the wrap passes");
        instructions.push(wrap(&request, request.amount, &wrapped_sol(&request.taker)));
        assert_not_allowed(
            check(&request, &instructions).expect_err("refused"),
            system_program::ID,
        );
    }

    #[test]
    fn wrapping_from_another_payer_is_refused() {
        let request = sol_request();
        let other = Pubkey::new_unique();
        let transfer = solana_system_interface::instruction::transfer(
            &other,
            &wrapped_sol(&request.taker),
            request.amount,
        );
        assert_not_allowed(refused_with(&request, transfer), system_program::ID);
    }

    #[test]
    fn compute_unit_price_is_capped() {
        let request = sol_request();
        let mut instructions = swap_instructions(&request);
        instructions[1] = price(MAX_COMPUTE_UNIT_PRICE);
        check(&request, &instructions).expect("at the cap");
        instructions[1] = price(MAX_COMPUTE_UNIT_PRICE + 1);
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::ComputeUnitPriceAboveCap { .. })
        ));
    }

    #[test]
    fn compute_unit_limit_is_capped() {
        let request = sol_request();
        let mut instructions = swap_instructions(&request);
        instructions[0] = limit(MAX_COMPUTE_UNIT_LIMIT);
        check(&request, &instructions).expect("at the cap");
        instructions[0] = limit(MAX_COMPUTE_UNIT_LIMIT + 1);
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::ComputeUnitLimitAboveCap { .. })
        ));
    }

    #[test]
    fn other_compute_budget_instructions_are_refused() {
        let request = sol_request();
        let mut instructions = swap_instructions(&request);
        instructions.push(
            compute_budget::ComputeBudgetInstruction::set_loaded_accounts_data_size_limit(65_536),
        );
        check(&request, &instructions).expect("a data size limit only restricts the transaction");
        let mut short_size =
            compute_budget::ComputeBudgetInstruction::set_loaded_accounts_data_size_limit(65_536);
        short_size.data.pop();
        assert_not_allowed(refused_with(&request, short_size), compute_budget::ID);
        let heap = compute_budget::ComputeBudgetInstruction::request_heap_frame(64 * 1024);
        assert_not_allowed(refused_with(&request, heap), compute_budget::ID);
        let mut short_price = price(1);
        short_price.data.pop();
        assert_not_allowed(refused_with(&request, short_price), compute_budget::ID);
        let mut short_limit = limit(1);
        short_limit.data.pop();
        assert_not_allowed(refused_with(&request, short_limit), compute_budget::ID);
    }

    #[test]
    fn associated_account_forms_are_limited() {
        let request = sol_request();
        let taker = request.taker;
        let other = Pubkey::new_unique();
        // Another wallet's account, paid for by the taker.
        assert_not_allowed(
            refused_with(&request, create_ata(&taker, &other, &MOBILE)),
            spl_associated_token_account::ID,
        );
        // The taker's account, paid for by someone else.
        assert_not_allowed(
            refused_with(&request, create_ata(&other, &taker, &MOBILE)),
            spl_associated_token_account::ID,
        );
        // The non-idempotent create.
        let mut create = create_ata(&taker, &taker, &MOBILE);
        create.data = vec![0];
        assert_not_allowed(
            refused_with(&request, create),
            spl_associated_token_account::ID,
        );
        // Under a token program that is neither SPL Token nor Token-2022.
        let mut create = create_ata(&taker, &taker, &MOBILE);
        create.accounts[5].pubkey = Pubkey::new_unique();
        assert_not_allowed(
            refused_with(&request, create),
            spl_associated_token_account::ID,
        );
        // Token-2022 is allowed.
        let mut instructions = swap_instructions(&request);
        instructions.push(
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &taker,
                &taker,
                &MOBILE,
                &TOKEN_2022,
            ),
        );
        check(&request, &instructions).expect("Token-2022 account");
    }

    #[test]
    fn token_instructions_are_limited_to_the_takers_wrapped_sol() {
        let request = sol_request();
        let taker = request.taker;
        let usdc = get_associated_token_address(&taker, &USDC);
        let other = Pubkey::new_unique();
        let sync = spl_token::instruction::sync_native(&spl_token::ID, &usdc).expect("sync");
        assert_not_allowed(refused_with(&request, sync), spl_token::ID);
        let close = |account: &Pubkey, destination: &Pubkey, owner: &Pubkey| {
            spl_token::instruction::close_account(&spl_token::ID, account, destination, owner, &[])
                .expect("close")
        };
        let wsol = wrapped_sol(&taker);
        assert_not_allowed(
            refused_with(&request, close(&usdc, &taker, &taker)),
            spl_token::ID,
        );
        assert_not_allowed(
            refused_with(&request, close(&wsol, &other, &taker)),
            spl_token::ID,
        );
        assert_not_allowed(
            refused_with(&request, close(&wsol, &taker, &other)),
            spl_token::ID,
        );
    }

    #[test]
    fn unknown_programs_are_refused() {
        let request = sol_request();
        let memo = Instruction::new_with_bytes(spl_memo::ID, b"hi", vec![]);
        assert!(matches!(
            refused_with(&request, memo),
            SwapRefusal::ProgramNotAllowed { program, .. } if program == spl_memo::ID
        ));
        let token_2022_transfer = Instruction::new_with_bytes(
            TOKEN_2022,
            &[3],
            vec![AccountMeta::new(request.taker, true)],
        );
        assert!(matches!(
            refused_with(&request, token_2022_transfer),
            SwapRefusal::ProgramNotAllowed { program, .. } if program == TOKEN_2022
        ));
    }

    /// The baseline with its route replaced by one carrying `data`.
    fn with_route(request: &SwapRequest, data: Vec<u8>) -> Result<ComputeBudget, SwapRefusal> {
        let mut instructions = swap_instructions(request);
        check(request, &instructions).expect("baseline passes");
        instructions[6] = route(&request.taker, data);
        check(request, &instructions)
    }

    #[test]
    fn route_must_spend_the_amount() {
        let request = sol_request();
        assert!(matches!(
            with_route(&request, route_data(request.amount + 1, QUOTED_OUT, 100)),
            Err(SwapRefusal::RouteInAmount { in_amount, .. }) if in_amount == request.amount + 1
        ));
    }

    #[test]
    fn route_minimum_must_reach_the_order_minimum() {
        let request = sol_request();
        // 1,000,000 less 100 bps is exactly the order minimum.
        with_route(&request, route_data(request.amount, QUOTED_OUT, 100)).expect("at the minimum");
        assert!(matches!(
            with_route(&request, route_data(request.amount, QUOTED_OUT, 101)),
            Err(SwapRefusal::RouteMinimumBelowOrder {
                route_minimum: 989_900,
                min_out: ORDER_MIN_OUT,
                ..
            })
        ));
        assert!(matches!(
            with_route(&request, route_data(request.amount, QUOTED_OUT - 1, 100)),
            Err(SwapRefusal::RouteMinimumBelowOrder {
                route_minimum: 989_999,
                ..
            })
        ));
    }

    #[test]
    fn shared_accounts_route_reads_past_its_id() {
        let request = sol_request();
        let taker = request.taker;
        let mut data = SHARED_ACCOUNTS_ROUTE_V2.to_vec();
        data.push(15);
        data.extend_from_slice(&route_data(request.amount, QUOTED_OUT, 100)[8..]);
        // The recorded layout: program authority, authority, source, the
        // program's source and destination, destination, then the rest.
        let shared = |data: &[u8], destination: Pubkey| {
            let mut ix = route(&taker, data.to_vec());
            ix.accounts[SHARED_ACCOUNTS_ROUTE_V2_DESTINATION].pubkey = destination;
            ix
        };
        let usdc = get_associated_token_address(&taker, &USDC);
        let mut instructions = swap_instructions(&request);
        instructions[6] = shared(&data, usdc);
        check(&request, &instructions).expect("shared accounts route");
        // Its optional-destination slot is not `route_v2`'s.
        instructions[6].accounts[ROUTE_V2_DESTINATION].pubkey = Pubkey::new_unique();
        check(&request, &instructions).expect("slot 7 is not a destination here");
        let other = Pubkey::new_unique();
        instructions[6] = shared(&data, other);
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::RouteDestination { account, .. }) if account == other
        ));
        data[9..17].copy_from_slice(&(request.amount - 1).to_le_bytes());
        instructions[6] = shared(&data, usdc);
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::RouteInAmount { .. })
        ));
    }

    #[test]
    fn other_jupiter_instructions_are_refused() {
        let request = sol_request();
        // `route`, whose arguments follow a variable-length route plan.
        let mut v1 = vec![0xe5, 0x17, 0xcb, 0x97, 0x7a, 0xe3, 0xad, 0x2a];
        v1.extend_from_slice(&route_data(request.amount, QUOTED_OUT, 100)[8..]);
        assert_not_allowed(
            with_route(&request, v1).expect_err("refused"),
            JUPITER_AGGREGATOR_V6,
        );
        let mut short = route_data(request.amount, QUOTED_OUT, 100);
        short.truncate(ROUTE_V2.len() + 17);
        assert_not_allowed(
            with_route(&request, short).expect_err("refused"),
            JUPITER_AGGREGATOR_V6,
        );
    }

    #[tokio::test]
    async fn recorded_route_with_its_slippage_widened_is_refused() {
        let mut case = case(HNT_USDC);
        let mut message = case.message();
        let VersionedMessage::V0(v0) = &mut message else {
            panic!("fixture is v0");
        };
        let route = v0
            .instructions
            .iter_mut()
            .find(|ix| ix.data.starts_with(&SHARED_ACCOUNTS_ROUTE_V2))
            .expect("route instruction");
        // id, in_amount, quoted_out_amount, then slippage_bps.
        route.data[25..27].copy_from_slice(&10_000u16.to_le_bytes());
        case.set_message(message);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::RouteMinimumBelowOrder {
                route_minimum: 0,
                ..
            }
        ));
    }

    #[test]
    fn fee_payer_must_be_the_taker() {
        let request = sol_request();
        let instructions = swap_instructions(&request);
        let message =
            VersionedMessage::Legacy(Message::new(&instructions, Some(&Pubkey::new_unique())));
        let keys = message.static_account_keys().to_vec();
        assert!(matches!(
            check_instructions(&message, &keys, &request, ORDER_MIN_OUT),
            Err(SwapRefusal::FeePayerNotTaker)
        ));
    }

    #[test]
    fn instruction_indexes_outside_the_keys_are_refused() {
        let request = sol_request();
        let mut message = Message::new(&swap_instructions(&request), Some(&request.taker));
        message.instructions[6].accounts.push(200);
        check_instructions(
            &VersionedMessage::Legacy(message.clone()),
            &message.account_keys,
            &request,
            ORDER_MIN_OUT,
        )
        .expect("the route's accounts are not inspected");
        // A wrap transfer whose destination is past the keys.
        message.instructions[3].accounts[1] = 200;
        let keys = message.account_keys.clone();
        assert!(matches!(
            check_instructions(
                &VersionedMessage::Legacy(message),
                &keys,
                &request,
                ORDER_MIN_OUT
            ),
            Err(SwapRefusal::AccountIndexOutOfRange { index: 3 })
        ));
    }

    #[test]
    fn fee_bound_covers_signatures_and_the_priority_fee() {
        let budget = ComputeBudget {
            limit: 274_672,
            price: 199_967,
        };
        // 199,967 µL × 274,672 CU = 54,925.6 lamports, rounded up.
        assert_eq!(budget.fee_bound(1), 5_000 + 54_926);
        assert_eq!(budget.fee_bound(2), 10_000 + 54_926);
    }

    // ---- simulation ----

    #[tokio::test]
    async fn failed_simulation_is_refused() {
        let mut case = case(HNT_USDC);
        case.chain.sim_err = Some(TransactionError::InsufficientFundsForFee);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::SimulationFailed(TransactionError::InsufficientFundsForFee)
        ));
    }

    #[tokio::test]
    async fn incomplete_simulation_is_refused() {
        let mut case = case(HNT_USDC);
        case.chain.drop_post = true;
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::SimulationIncomplete
        ));
    }

    #[tokio::test]
    async fn output_below_the_threshold_is_refused() {
        let mut case = case(HNT_USDC);
        let usdc = case.token_account_of(&USDC);
        let threshold = 350_386;
        let floor = case.token_amount(&case.chain.pre, &usdc) + threshold;
        case.edit_post_token(&usdc, |token| token.amount = floor);
        case.run().await.expect("exactly the threshold passes");
        case.edit_post_token(&usdc, |token| token.amount = floor - 1);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::OutputBelowThreshold {
                threshold: 350_386,
                received: 350_385
            }
        ));
    }

    #[tokio::test]
    async fn input_spent_other_than_the_amount_is_refused() {
        for change in [-1i64, 1] {
            let mut case = case(HNT_USDC);
            let hnt = case.token_account_of(&HNT);
            let post = case.token_amount(&case.chain.post, &hnt);
            case.edit_post_token(&hnt, |token| {
                token.amount = post.checked_add_signed(change).expect("amount")
            });
            assert!(matches!(
                case.refusal().await,
                SwapRefusal::InputSpent {
                    expected: 100_000_000,
                    ..
                }
            ));
        }
    }

    #[tokio::test]
    async fn lamports_beyond_the_fee_bound_are_refused() {
        for fixture in [HNT_USDC, SOL_USDC] {
            let mut case = case(fixture);
            let taker = case.request.taker;
            case.chain.post.get_mut(&taker).expect("taker").lamports -= 1;
            assert!(matches!(
                case.refusal().await,
                SwapRefusal::LamportsOverBound { .. }
            ));
        }
    }

    #[tokio::test]
    async fn taker_account_reassigned_or_given_data_is_refused() {
        let mut reassigned = case(HNT_USDC);
        let taker = reassigned.request.taker;
        reassigned.chain.post.get_mut(&taker).expect("taker").owner = Pubkey::new_unique();
        assert!(matches!(
            reassigned.refusal().await,
            SwapRefusal::TakerAccountChanged
        ));

        let mut allocated = case(HNT_USDC);
        allocated.chain.post.get_mut(&taker).expect("taker").data = vec![0; 8];
        assert!(matches!(
            allocated.refusal().await,
            SwapRefusal::TakerAccountChanged
        ));

        let mut closed = case(HNT_USDC);
        closed.chain.post.remove(&taker);
        assert!(matches!(
            closed.refusal().await,
            SwapRefusal::TakerAccountChanged
        ));
    }

    #[tokio::test]
    async fn token_account_owner_change_is_refused() {
        let mut case = case(HNT_USDC);
        let usdc = case.token_account_of(&USDC);
        case.edit_post_token(&usdc, |token| token.owner = Pubkey::new_unique());
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::TokenOwnerChanged { account } if account == usdc
        ));
    }

    #[tokio::test]
    async fn token_account_delegate_or_close_authority_is_refused() {
        let mut case_delegate = case(HNT_USDC);
        let hnt = case_delegate.token_account_of(&HNT);
        case_delegate.edit_post_token(&hnt, |token| {
            token.delegate = COption::Some(Pubkey::new_unique())
        });
        assert!(matches!(
            case_delegate.refusal().await,
            SwapRefusal::TokenAuthorityChanged { account } if account == hnt
        ));

        let mut case_close = case(HNT_USDC);
        case_close.edit_post_token(&hnt, |token| {
            token.close_authority = COption::Some(Pubkey::new_unique())
        });
        assert!(matches!(
            case_close.refusal().await,
            SwapRefusal::TokenAuthorityChanged { account } if account == hnt
        ));
    }

    /// A writable account the recorded state does not mention.
    fn untouched_writable(case: &Case) -> Pubkey {
        case.resolved()
            .writable
            .into_iter()
            .find(|key| !case.chain.pre.contains_key(key) && !case.chain.post.contains_key(key))
            .expect("an untouched writable account")
    }

    #[tokio::test]
    async fn new_taker_account_with_a_close_authority_is_refused() {
        let mut case = case(HNT_USDC);
        let address = untouched_writable(&case);
        let mut account = token_account_state(MOBILE, case.request.taker, 0);
        edit_token(&mut account, |token| {
            token.close_authority = COption::Some(Pubkey::new_unique())
        });
        case.chain.post.insert(address, account);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::TokenAuthorityChanged { account } if account == address
        ));
    }

    /// A writable account the transaction reaches through a lookup table.
    fn lookup_writable(case: &Case) -> Pubkey {
        let static_keys = case.message().static_account_keys().to_vec();
        case.resolved()
            .writable
            .into_iter()
            .find(|key| !static_keys.contains(key) && !case.chain.pre.contains_key(key))
            .expect("a lookup-table writable account")
    }

    #[tokio::test]
    async fn taker_accounts_reached_through_lookup_tables_are_compared() {
        let mut case = case(HNT_USDC);
        let address = lookup_writable(&case);
        let taker = case.request.taker;
        case.chain
            .pre
            .insert(address, token_account_state(MOBILE, taker, 10));
        case.chain
            .post
            .insert(address, token_account_state(MOBILE, taker, 9));
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::OtherTokenDecreased { mint } if mint == MOBILE
        ));
    }

    #[tokio::test]
    async fn accounts_the_taker_does_not_own_are_not_its_balance() {
        // A pool's output vault paying out is not the taker losing output.
        let mut case = case(HNT_USDC);
        let address = untouched_writable(&case);
        let pool = Pubkey::new_unique();
        case.chain
            .pre
            .insert(address, token_account_state(USDC, pool, 1_000_000_000));
        case.chain
            .post
            .insert(address, token_account_state(USDC, pool, 1));
        case.run()
            .await
            .expect("a pool's balance is not the taker's");
    }

    #[tokio::test]
    async fn unwrapping_the_takers_wrapped_sol_is_not_a_token_loss() {
        // An intermediate wrapped-SOL account closed back to the taker
        // moves its lamports into the taker's account.
        let mut case = case(HNT_USDC);
        let address = untouched_writable(&case);
        let taker = case.request.taker;
        let mut wsol = token_account_state(native_mint::ID, taker, 5_000_000);
        wsol.lamports += 5_000_000;
        let lamports = wsol.lamports;
        case.chain.pre.insert(address, wsol);
        case.chain.post.insert(
            address,
            Account {
                lamports: 0,
                ..Account::default()
            },
        );
        case.chain.post.get_mut(&taker).expect("taker").lamports += lamports;
        case.run().await.expect("unwrapping is value-neutral");
    }

    #[tokio::test]
    async fn multi_signer_transaction_is_refused_before_signing() {
        let mut case = case(HNT_USDC);
        let mut message = case.message();
        let VersionedMessage::V0(v0) = &mut message else {
            panic!("fixture is v0");
        };
        v0.header.num_required_signatures = 2;
        case.set_message(message);
        assert!(matches!(
            case.run().await,
            Err(JupiterError::MultipleSigners { num_signers: 2, .. })
        ));
    }

    #[tokio::test]
    async fn new_taker_account_with_a_delegate_is_refused() {
        let mut case = case(HNT_USDC);
        let address = untouched_writable(&case);
        let mut account = token_account_state(MOBILE, case.request.taker, 0);
        case.chain.post.insert(address, account.clone());
        case.run().await.expect("a new plain taker account passes");
        edit_token(&mut account, |token| {
            token.delegate = COption::Some(Pubkey::new_unique())
        });
        case.chain.post.insert(address, account);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::TokenAuthorityChanged { account } if account == address
        ));
    }

    #[tokio::test]
    async fn other_taker_balance_decreasing_is_refused() {
        let mut case = case(HNT_USDC);
        let address = untouched_writable(&case);
        let taker = case.request.taker;
        case.chain
            .pre
            .insert(address, token_account_state(MOBILE, taker, 10));
        case.chain
            .post
            .insert(address, token_account_state(MOBILE, taker, 10));
        case.run().await.expect("an unchanged balance passes");
        case.chain
            .post
            .insert(address, token_account_state(MOBILE, taker, 9));
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::OtherTokenDecreased { mint } if mint == MOBILE
        ));
    }

    #[test]
    fn sol_output_must_reach_the_threshold_net_of_fees() {
        let taker = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let request = SwapRequest {
            input_mint: USDC,
            output_mint: native_mint::ID,
            amount: 1_000_000,
            min_output: 10_000_000,
            slippage_bps: 100,
            taker,
        };
        let expected = ExpectedEffects {
            request,
            min_out: 10_000_000,
            fee_bound: 60_000,
        };
        let writable = [taker, usdc];
        let lamports = |lamports| Account {
            lamports,
            ..Account::default()
        };
        let pre = [
            Some(lamports(1_000_000_000)),
            Some(token_account_state(USDC, taker, 5_000_000)),
        ];
        let post = |received: u64| {
            [
                Some(lamports(1_000_000_000 + received - 60_000)),
                Some(token_account_state(USDC, taker, 4_000_000)),
            ]
        };
        check_effects(&expected, &writable, &pre, &post(10_000_000)).expect("at the threshold");
        assert!(matches!(
            check_effects(&expected, &writable, &pre, &post(9_999_999)),
            Err(SwapRefusal::OutputBelowThreshold {
                received: 9_999_999,
                ..
            })
        ));
    }

    #[test]
    fn token_account_detection() {
        let taker = Pubkey::new_unique();
        let mut account = token_account_state(USDC, taker, 1);
        assert!(token_account(&account).is_some());
        // A Token-2022 account carrying extensions.
        account.owner = TOKEN_2022;
        account
            .data
            .extend_from_slice(&[TOKEN_2022_ACCOUNT_TYPE, 0, 0]);
        assert!(token_account(&account).is_some());
        // A Token-2022 mint carrying extensions.
        account.data[TokenAccount::LEN] = 1;
        assert!(token_account(&account).is_none());
        // A multisig, whose byte at that offset is arbitrary.
        account.data.resize(Multisig::LEN, 0);
        account.data[TokenAccount::LEN] = TOKEN_2022_ACCOUNT_TYPE;
        assert!(token_account(&account).is_none());
        // Owned by another program.
        let mut foreign = token_account_state(USDC, taker, 1);
        foreign.owner = Pubkey::new_unique();
        assert!(token_account(&foreign).is_none());
    }

    // ---- the caller's minimum output ----

    #[test]
    fn order_minimum_must_reach_the_callers_minimum() {
        let Case {
            order, mut request, ..
        } = case(HNT_USDC);
        request.min_output = 350_386;
        assert_eq!(
            check_order(&order, &request).expect("at the caller's minimum"),
            350_386
        );
        request.min_output = 350_387;
        assert!(matches!(
            check_order(&order, &request),
            Err(SwapRefusal::ThresholdBelowMinimum {
                threshold: 350_386,
                min_output: 350_387
            })
        ));
    }

    #[tokio::test]
    async fn zero_quote_is_refused_unsigned() {
        let mut case = case(HNT_USDC);
        case.order.out_amount = "0".to_string();
        case.order.other_amount_threshold = Some("0".to_string());
        let (result, signed) = case.run_counting().await;
        assert!(
            matches!(
                result,
                Err(JupiterError::Refused(SwapRefusal::OutAmountZero))
            ),
            "{result:?}"
        );
        assert_eq!(signed, 0);
    }

    #[tokio::test]
    async fn quote_whose_minimum_is_zero_is_refused() {
        // A 100% slippage order: a real quote, a zero minimum.
        let mut case = case(HNT_USDC);
        case.order.slippage_bps = 10_000;
        case.request.slippage_bps = 10_000;
        case.order.other_amount_threshold = Some("0".to_string());
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::ThresholdBelowMinimum { threshold: 0, .. }
        ));
    }

    #[test]
    fn order_mismatch_text_is_bounded() {
        let refusal =
            order_field(|order, _| order.taker = Some(format!("\u{1b}[2J{}", "x".repeat(1_000))));
        let SwapRefusal::OrderMismatch { actual, .. } = refusal else {
            panic!("expected OrderMismatch, got {refusal:?}");
        };
        assert!(!actual.contains('\u{1b}'));
        assert!(actual.starts_with("\\u{1b}[2J"));
        assert!(actual.chars().count() <= super::super::MAX_ERROR_BODY_LEN + 8);
    }

    // ---- signing ----

    #[tokio::test]
    async fn refused_transaction_is_never_signed() {
        let mut case = case(HNT_USDC);
        let taker = case.request.taker;
        case.chain.post.get_mut(&taker).expect("taker").lamports -= 1;
        let (result, signed) = case.run_counting().await;
        assert!(matches!(
            result,
            Err(JupiterError::Refused(SwapRefusal::LamportsOverBound { .. }))
        ));
        assert_eq!(signed, 0);

        let mut failed = self::case(SOL_USDC);
        failed.chain.sim_err = Some(TransactionError::InsufficientFundsForFee);
        let (result, signed) = failed.run_counting().await;
        assert!(matches!(
            result,
            Err(JupiterError::Refused(SwapRefusal::SimulationFailed(_)))
        ));
        assert_eq!(signed, 0);
    }

    // ---- compute unit limit ----

    #[test]
    fn exactly_one_compute_unit_limit_is_required() {
        let request = sol_request();
        let mut instructions = swap_instructions(&request);
        instructions.remove(0);
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::ComputeUnitLimitCount { count: 0 })
        ));
        let mut instructions = swap_instructions(&request);
        instructions.insert(0, limit(1));
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::ComputeUnitLimitCount { count: 2 })
        ));
    }

    // ---- route destination and platform fee ----

    #[test]
    fn route_must_deliver_to_the_takers_output_account() {
        let request = sol_request();
        let data = route_data(request.amount, QUOTED_OUT, 100);
        let mut instructions = swap_instructions(&request);
        let other = Pubkey::new_unique();
        instructions[6].accounts[ROUTE_V2_USER_DESTINATION].pubkey = other;
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::RouteDestination { index: 6, account }) if account == other
        ));
        // The taker's own account of another mint is not the output account.
        let mut instructions = swap_instructions(&request);
        let hnt = get_associated_token_address(&request.taker, &HNT);
        instructions[6].accounts[ROUTE_V2_USER_DESTINATION].pubkey = hnt;
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::RouteDestination { account, .. }) if account == hnt
        ));
        // The optional destination must be left empty.
        let mut instructions = swap_instructions(&request);
        let usdc = get_associated_token_address(&request.taker, &USDC);
        instructions[6].accounts[ROUTE_V2_DESTINATION].pubkey = usdc;
        assert!(matches!(
            check(&request, &instructions),
            Err(SwapRefusal::RouteDestination { account, .. }) if account == usdc
        ));
        // The taker's Token-2022 account of the output mint is accepted.
        let mut instructions = swap_instructions(&request);
        instructions[6] = route(&request.taker, data);
        instructions[6].accounts[ROUTE_V2_USER_DESTINATION].pubkey =
            get_associated_token_address_with_program_id(&request.taker, &USDC, &TOKEN_2022);
        check(&request, &instructions).expect("Token-2022 output account");
    }

    #[tokio::test]
    async fn recorded_routes_redirected_elsewhere_are_refused() {
        // route_v2 (SOL fixture): the user destination, then the optional one.
        for (fixture, discriminator, slot) in [
            (SOL_USDC, ROUTE_V2, ROUTE_V2_USER_DESTINATION),
            (SOL_USDC, ROUTE_V2, ROUTE_V2_DESTINATION),
            (
                HNT_USDC,
                SHARED_ACCOUNTS_ROUTE_V2,
                SHARED_ACCOUNTS_ROUTE_V2_DESTINATION,
            ),
        ] {
            let mut case = case(fixture);
            let mut message = case.message();
            let VersionedMessage::V0(v0) = &mut message else {
                panic!("fixture is v0");
            };
            let route = v0
                .instructions
                .iter_mut()
                .find(|ix| ix.data.starts_with(&discriminator))
                .expect("route instruction");
            // Point the slot at the route's source account.
            let source = if discriminator == ROUTE_V2 { 1 } else { 2 };
            route.accounts[slot] = route.accounts[source];
            case.set_message(message);
            let (result, signed) = case.run_counting().await;
            assert!(
                matches!(
                    result,
                    Err(JupiterError::Refused(SwapRefusal::RouteDestination { .. }))
                ),
                "{result:?}"
            );
            assert_eq!(signed, 0);
        }
    }

    #[test]
    fn route_platform_fee_is_capped() {
        let request = sol_request();
        let data = route_data(request.amount, QUOTED_OUT, 100);
        with_route(
            &request,
            with_platform_fee(data.clone(), MAX_PLATFORM_FEE_BPS),
        )
        .expect("at the cap");
        assert!(matches!(
            with_route(&request, with_platform_fee(data, MAX_PLATFORM_FEE_BPS + 1)),
            Err(SwapRefusal::PlatformFeeAboveCap {
                fee_bps: 51,
                cap: 50,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn recorded_shared_route_with_its_fee_raised_is_refused() {
        let mut case = case(HNT_USDC);
        let mut message = case.message();
        let VersionedMessage::V0(v0) = &mut message else {
            panic!("fixture is v0");
        };
        let route = v0
            .instructions
            .iter_mut()
            .find(|ix| ix.data.starts_with(&SHARED_ACCOUNTS_ROUTE_V2))
            .expect("route instruction");
        // id, in_amount, quoted_out_amount, slippage_bps, then the fee.
        assert_eq!(route.data[27..29], 10u16.to_le_bytes());
        route.data[27..29].copy_from_slice(&(MAX_PLATFORM_FEE_BPS + 1).to_le_bytes());
        case.set_message(message);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::PlatformFeeAboveCap { fee_bps: 51, .. }
        ));
    }

    // ---- accounts reached through lookup tables ----

    /// `instructions` compiled against one lookup table holding `addresses`,
    /// then checked with the table resolved as `sign_verified` resolves it.
    fn check_v0(
        request: &SwapRequest,
        instructions: &[Instruction],
        addresses: Vec<Pubkey>,
    ) -> Result<ComputeBudget, SwapRefusal> {
        let table = Pubkey::new_unique();
        let entries = addresses
            .iter()
            .enumerate()
            .map(|(index, address)| (index.to_string(), address.to_string()))
            .collect();
        let message = VersionedMessage::V0(
            v0::Message::try_compile(
                &request.taker,
                instructions,
                &[AddressLookupTableAccount {
                    key: table,
                    addresses,
                }],
                Hash::default(),
            )
            .expect("compile"),
        );
        let lookups = message.address_table_lookups().expect("v0");
        assert_eq!(lookups.len(), 1, "the table is used");
        let resolved =
            resolve_accounts(&message, &[Some(lookup_table(&entries))]).expect("resolve");
        check_instructions(&message, &resolved.keys, request, ORDER_MIN_OUT)
    }

    #[test]
    fn guarded_accounts_reached_through_lookup_tables_are_checked() {
        let request = sol_request();
        let taker = request.taker;
        let wsol = wrapped_sol(&taker);
        let other = Pubkey::new_unique();
        // The taker's wrapped SOL (loaded writable) and the mints (loaded
        // readonly) resolve to themselves, so the baseline passes; resolving
        // loaded readonly before loaded writable would misplace them.
        let table = vec![wsol, USDC, native_mint::ID, other];
        check_v0(&request, &swap_instructions(&request), table.clone())
            .expect("baseline through the table passes");

        // A CloseAccount destination reached as a loaded writable account.
        let mut instructions = swap_instructions(&request);
        instructions[7] =
            spl_token::instruction::close_account(&spl_token::ID, &wsol, &other, &taker, &[])
                .expect("close");
        assert_not_allowed(
            check_v0(&request, &instructions, table.clone()).expect_err("refused"),
            spl_token::ID,
        );

        // A System transfer target reached as a loaded writable account.
        let mut instructions = swap_instructions(&request);
        instructions[3] = wrap(&request, request.amount, &other);
        assert_not_allowed(
            check_v0(&request, &instructions, table.clone()).expect_err("refused"),
            system_program::ID,
        );

        // An associated account's wallet reached as a loaded readonly account.
        let mut instructions = swap_instructions(&request);
        instructions[5] = create_ata(&taker, &other, &USDC);
        assert_not_allowed(
            check_v0(&request, &instructions, table).expect_err("refused"),
            spl_associated_token_account::ID,
        );
    }

    // ---- RPC errors ----

    #[tokio::test]
    async fn rpc_errors_carry_no_url() {
        let rpc = SolanaRpcClient::new("http://127.0.0.1:1/?api-key=SECRET-KEY".to_string());
        let err = SwapChain::latest_blockhash(&rpc)
            .await
            .expect_err("nothing listens on port 1");
        assert!(matches!(err, JupiterError::Solana(_)), "{err:?}");
        let mut text = format!("{err} {err:?}");
        let mut source = std::error::Error::source(&err);
        while let Some(cause) = source {
            text.push_str(&format!(" {cause} {cause:?}"));
            source = cause.source();
        }
        assert!(
            text.contains("Connect") || text.contains("connect"),
            "{text}"
        );
        assert!(!text.contains("SECRET-KEY"), "{text}");
        assert!(!text.contains("api-key"), "{text}");
    }
}
