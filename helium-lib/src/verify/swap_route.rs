//! Checks that a swap quote and the transaction built for it have the
//! outcome the caller asked for, before the taker signs it.
//! [`crate::swap::prepare`] is the entry point that runs them, and
//! [`crate::swap::PreparedSwap::sign`] signs what they passed.
//!
//! The checks guard against a builder that is buggy or misconfigured, not a
//! hostile one. They judge one simulation of the transaction, so a program
//! that behaves differently when the transaction lands (one upgraded after
//! the simulation, or one that branches on the slot or on an account's
//! contents) is not caught: the taker's signature goes to every program the
//! transaction invokes directly, and whatever such a program does in the
//! landed transaction is not checked.
//!
//! The checks judge what the simulated transaction does to the taker, not
//! how the route is built: any program, instruction form or call depth the
//! builder chooses passes when the simulated outcome is the requested swap.
//! They are:
//!
//! 1. [`check_signer`]: the taker is the transaction's fee payer and only
//!    signer.
//! 2. [`check_quote`]: the quote names the requested mints, amount,
//!    slippage and an exact-in swap, quotes a non-zero output, and its
//!    minimum output is at least the caller's `min_output` and no lower than
//!    the slippage allows on the quote.
//! 3. [`fee_bound`]: the transaction sets exactly one compute unit limit,
//!    and the compute unit price is at most [`MAX_COMPUTE_UNIT_PRICE`],
//!    which bounds the SOL the transaction's fee can take. Precompile
//!    signatures are not counted; the runtime charges for them, so a
//!    transaction carrying one can spend more than the bound and be
//!    refused, never accepted past it.
//! 4. [`check_route_minimum`]: a top-level Jupiter `route_v2` or
//!    `shared_accounts_route_v2` instruction carries a minimum output, its
//!    quote less its slippage and platform fee, of at least the caller's
//!    `min_output`. This is what binds the landed swap, since the program
//!    enforces the route's own minimum, which the builder sets. Every other
//!    instruction form, and Jupiter reached through another program, is not
//!    read.
//! 5. [`check_size`]: the unsigned transaction fits in one packet,
//!    [`PACKET_DATA_SIZE`] bytes. A larger one cannot be sent, and the RPC
//!    node rejects it when asked to simulate it, an error a caller cannot
//!    tell from an outage, so it is refused first with its own variant.
//! 6. [`check_effects`]: a simulation of the unsigned transaction shows the
//!    taker spending exactly `amount` of the input mint, receiving at least
//!    the quote's minimum output into its own token accounts, losing no
//!    other token, and losing no more SOL (its own lamports plus those of its
//!    token accounts) than the fee bound, plus `amount` when the input is
//!    SOL. The taker's own account keeps its owner and data, every taker
//!    token account keeps its owner, delegate, delegated amount, close
//!    authority and state, and a token account the taker gains has neither
//!    a delegate nor a close authority.
//! 7. [`check_hotspot_programs`]: no instruction of the Bubblegum or
//!    lazy distributor program, at the top level or beneath it at any
//!    depth, names the taker among its accounts. Bubblegum holds the
//!    wallet's compressed NFTs, which hold its hotspots, and the lazy
//!    distributor sets where their rewards go; a swap has no reason to call
//!    either.
//!
//! Not checked: which programs the transaction invokes, the forms of their
//! instructions, route arguments other than the minimum above, and state
//! that programs other than Bubblegum and the lazy distributor keep for the
//! taker (stake and vote authorities, nonce account authorities, program
//! upgrade authorities, Token Metadata and MPL Core asset authorities,
//! other rewards recipients, staked positions, multisig membership). A
//! program the transaction hands the taker's signature can change that state
//! without any effect the checks compare.
//!
//! The simulation runs on the caller's own RPC node, not on the service that
//! built the transaction. The taker signs only after every check passes, so
//! a refused transaction is never signed and the RPC node that simulates it
//! never holds a signed copy.

use super::{compute_budget_settings, JUPITER_AGGREGATOR_V6, MAX_COMPUTE_UNIT_LIMIT};
use crate::{
    blockchain_api::{bounded_text, types::SwapQuote, BlockchainApiError},
    client::SolanaRpcClient,
    error::DecodeError,
    keypair::Pubkey,
    priority_fee::MAX_PRIORITY_FEE,
    programs,
    solana_client::{
        client_error::{ClientError, ClientErrorKind},
        rpc_config::{
            RpcAccountInfoConfig, RpcSimulateTransactionAccountsConfig,
            RpcSimulateTransactionConfig,
        },
    },
    solana_sdk::{
        account::Account,
        hash::Hash,
        message::VersionedMessage,
        packet::PACKET_DATA_SIZE,
        program_pack::Pack,
        pubkey,
        signature::Signature,
        signer::SignerError,
        transaction::{TransactionError, VersionedTransaction},
    },
    swap::SwapRequest,
};
use anchor_client::solana_account_decoder::UiAccountEncoding;
use anchor_spl::token::spl_token::{
    self, native_mint,
    state::{Account as TokenAccount, Multisig},
};
use serde_json::Value;
use solana_address_lookup_table_interface::{
    program as address_lookup_table, state::AddressLookupTable,
};
use solana_transaction_status::{
    UiInnerInstructions, UiInstruction, UiParsedInstruction, UiTransactionEncoding,
};
use std::collections::{BTreeSet, HashMap};

/// The Token-2022 program, which the taker's token accounts may belong to.
const TOKEN_2022: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
/// The highest compute unit price a swap transaction may set, in
/// micro-lamports: the crate's priority fee ceiling,
/// [`MAX_PRIORITY_FEE`], which governs every transaction the wallet prices or
/// accepts. At the maximum limit this caps the priority fee at 3,500,000
/// lamports (0.0035 SOL).
pub(crate) const MAX_COMPUTE_UNIT_PRICE: u64 = MAX_PRIORITY_FEE;
/// The base fee per signature, in lamports.
const LAMPORTS_PER_SIGNATURE: u64 = 5_000;
const MICRO_LAMPORTS_PER_LAMPORT: u128 = 1_000_000;
/// `AccountType::Account`, the byte Token-2022 writes after the base layout
/// of a token account that carries extensions.
const TOKEN_2022_ACCOUNT_TYPE: u8 = 2;
/// Jupiter aggregator `route_v2` and `shared_accounts_route_v2`, the
/// exact-in routes built when the quote asks for `instructionVersion` V2.
/// The program's published Anchor IDL gives both the arguments `in_amount:
/// u64`, `quoted_out_amount: u64`, `slippage_bps: u16` and `platform_fee_bps:
/// u16` at a fixed offset after the discriminator (`shared_accounts_route_v2`
/// first has an `id: u8`).
const ROUTE_V2: [u8; 8] = [0xbb, 0x64, 0xfa, 0xcc, 0x31, 0xc4, 0xaf, 0x14];
const SHARED_ACCOUNTS_ROUTE_V2: [u8; 8] = [0xd1, 0x98, 0x53, 0x93, 0x7c, 0xfe, 0xd8, 0xe9];
/// `getMultipleAccounts` answers at most this many keys per call.
const MAX_ACCOUNTS_PER_REQUEST: usize = 100;
/// The most characters of quote text a refusal carries.
const MAX_ERROR_BODY_LEN: usize = 200;

/// Why a swap was not signed: the request cannot describe a swap, a request
/// to the building service or a chain read failed, the transaction is
/// unusable, or a check refused it.
#[derive(Debug, thiserror::Error)]
pub enum SwapRouteError {
    /// The request cannot describe a swap.
    #[error(transparent)]
    InvalidRequest(#[from] InvalidSwapRequest),
    /// A request to the service that quotes and builds the swap failed.
    #[error("swap request to the blockchain-api failed")]
    Api(#[source] BlockchainApiError),
    /// The returned transaction does not decode.
    #[error("failed to decode the swap transaction")]
    TransactionDecode(#[source] DecodeError),
    /// A Solana RPC call failed. The source carries no RPC URL, since an
    /// RPC URL can carry an API key.
    #[error("Solana RPC request failed")]
    Solana(#[source] Box<ClientError>),
    /// The keypair could not name its wallet or sign.
    #[error("transaction signing failed")]
    Signing(#[source] SignerError),
    /// The keypair offered to sign a prepared swap is not the wallet the
    /// swap was built and checked for, so it was not asked to sign.
    #[error("swap refused: signer {signer} is not {taker}, the wallet the swap was checked for")]
    SignerNotTaker { signer: Pubkey, taker: Pubkey },
    /// The quote or its transaction does something beyond the requested
    /// swap, so it was not signed.
    #[error(transparent)]
    Refused(#[from] SwapRefusal),
}

/// A request that cannot describe a swap, refused before any request is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSwapRequest {
    #[error("swap amount must be greater than zero")]
    ZeroAmount,
    #[error("minimum swap output must be greater than zero")]
    ZeroMinOutput,
    #[error("input and output tokens must be different")]
    SameMint,
}

impl SwapRouteError {
    /// Wrap an RPC error with any URL stripped from its HTTP source, since
    /// an RPC URL can carry an API key.
    pub(crate) fn solana(err: ClientError) -> Self {
        let ClientError { request, kind } = err;
        let kind = match kind {
            ClientErrorKind::Reqwest(err) => ClientErrorKind::Reqwest(err.without_url()),
            other => other,
        };
        Self::Solana(Box::new(ClientError { request, kind }))
    }

    /// Wrap a blockchain-api error with any URL stripped from its HTTP
    /// source, as [`SwapRouteError::solana`] does for RPC errors.
    pub(crate) fn api(err: BlockchainApiError) -> Self {
        Self::Api(match err {
            BlockchainApiError::Request(err) => BlockchainApiError::Request(err.without_url()),
            other => other,
        })
    }
}

/// Refuse a transaction unless the taker is its only signer, which also makes
/// the taker its fee payer, since the fee payer is the first signer. A
/// transaction needing any other signature (a market maker's co-signature,
/// say) is one the taker alone cannot complete, and one whose fee payer is
/// another wallet was built for that wallet.
pub(crate) fn check_signer(message: &VersionedMessage, taker: &Pubkey) -> Result<(), SwapRefusal> {
    let num_signers = message.header().num_required_signatures;
    if num_signers != 1 {
        let extra_signers = message
            .static_account_keys()
            .iter()
            .take(usize::from(num_signers))
            .filter(|key| *key != taker)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(SwapRefusal::SignerCount {
            num_signers,
            extra_signers,
        });
    }
    if message.static_account_keys().first() != Some(taker) {
        return Err(SwapRefusal::FeePayerNotTaker);
    }
    Ok(())
}

/// Why a swap quote or its transaction was refused. Each variant names the
/// check that failed; none carries a secret or a URL.
#[derive(Debug, thiserror::Error)]
pub enum SwapRefusal {
    /// `actual` is response text, bounded by `bounded_text`.
    #[error("swap refused: quote {field} is {actual}, requested {expected}")]
    QuoteMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("swap refused: the quote has no output")]
    OutAmountZero,
    #[error("swap refused: quote minimum output {threshold} is below the requested minimum {min_output}")]
    ThresholdBelowMinimum { threshold: u64, min_output: u64 },
    #[error("swap refused: quote {field} is missing or malformed")]
    QuoteMalformed { field: &'static str },
    #[error("swap refused: quote minimum output {threshold} is below {min_out}, the least {slippage_bps} bps of slippage allows on {out_amount}")]
    ThresholdBelowSlippage {
        threshold: u64,
        min_out: u64,
        out_amount: u64,
        slippage_bps: u16,
    },
    #[error("swap refused: {found} transactions were returned, not one")]
    TransactionCount { found: usize },
    /// `extra_signers` lists the required signers other than the taker.
    #[error("swap refused: the transaction requires {num_signers} signatures, not the taker's alone (others: {extra_signers})")]
    SignerCount {
        num_signers: u8,
        extra_signers: String,
    },
    #[error("swap refused: the transaction's fee payer is not the taker")]
    FeePayerNotTaker,
    /// `size` is the unsigned transaction's serialized length in bytes and
    /// `limit` the largest a transaction may be, one packet. A route with
    /// many hops can exceed it; asking the builder again may return a
    /// smaller one.
    #[error("swap refused: the transaction is {size} bytes, over the {limit} byte limit")]
    TransactionTooLarge { size: u64, limit: u64 },
    #[error("swap refused: lookup table {table} could not be resolved")]
    LookupTableUnresolved { table: Pubkey },
    #[error(
        "swap refused: instruction {index} references an account the transaction does not hold"
    )]
    AccountIndexOutOfRange { index: usize },
    #[error("swap refused: the transaction sets the compute unit limit {count} times, not once")]
    ComputeUnitLimitCount { count: usize },
    #[error("swap refused: compute unit price {price} exceeds {cap} micro-lamports")]
    ComputeUnitPriceAboveCap { price: u64, cap: u64 },
    /// `route_minimum` is the least the route guarantees the taker, after
    /// its platform fee.
    #[error("swap refused: route instruction {index} guarantees {route_minimum} after its platform fee, below the requested minimum {min_output}")]
    RouteMinimumBelowRequest {
        index: usize,
        route_minimum: u64,
        min_output: u64,
    },
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
    #[error("swap refused: taker token account {account} changed its delegated amount or state")]
    TokenDelegationChanged { account: Pubkey },
    /// An instruction of Bubblegum, which can move the taker's compressed
    /// NFTs, or of the lazy distributor, which can redirect their rewards,
    /// names the taker.
    #[error("swap refused: the transaction passes the wallet to {program}, which holds its hotspots or their rewards")]
    HotspotProgramInstruction { program: Pubkey },
    #[error("swap refused: simulation returned an inner instruction that cannot be read")]
    InnerInstructionUnreadable,
}

/// The swap the caller asked for and the wallet that takes it: what the
/// quote and the transaction are checked against.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SwapTerms {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount: u64,
    /// The caller's floor on the output, in the output mint's smallest
    /// units.
    pub min_output: u64,
    pub slippage_bps: u16,
    pub taker: Pubkey,
}

impl SwapTerms {
    /// Terms for `taker` swapping `amount` of `input_mint` into at least
    /// `min_output` of `output_mint`, refused when the amount or the
    /// minimum is zero or the mints are the same.
    pub fn new(request: &SwapRequest, taker: Pubkey) -> Result<Self, InvalidSwapRequest> {
        if request.amount == 0 {
            return Err(InvalidSwapRequest::ZeroAmount);
        }
        if request.min_output == 0 {
            return Err(InvalidSwapRequest::ZeroMinOutput);
        }
        if request.input_mint == request.output_mint {
            return Err(InvalidSwapRequest::SameMint);
        }
        Ok(Self {
            input_mint: request.input_mint,
            output_mint: request.output_mint,
            amount: request.amount,
            min_output: request.min_output,
            slippage_bps: request.slippage_bps,
            taker,
        })
    }
}

/// A simulation's outcome: its error, if any, the post-state of the
/// accounts it was asked for, in the order they were asked for, and the
/// instructions it ran beneath the top level (`None` when the node did not
/// report them).
pub(crate) struct Simulation {
    pub err: Option<TransactionError>,
    pub accounts: Vec<Option<Account>>,
    pub inner_instructions: Option<Vec<UiInnerInstructions>>,
}

/// Accounts read from the chain, and the slot the node answered from (the
/// latest, when the read took more than one request).
pub(crate) struct AccountsRead {
    pub slot: u64,
    pub accounts: Vec<Option<Account>>,
}

/// The chain reads a swap needs, behind a trait so the checks run against
/// recorded state in tests. Each read that takes a `min_context_slot` is
/// answered from that slot or a later one.
pub(crate) trait SwapChain {
    async fn latest_blockhash(&self) -> Result<(Hash, u64), SwapRouteError>;
    async fn accounts(
        &self,
        keys: &[Pubkey],
        min_context_slot: u64,
    ) -> Result<AccountsRead, SwapRouteError>;
    async fn simulate(
        &self,
        txn: &VersionedTransaction,
        keys: &[Pubkey],
        min_context_slot: u64,
    ) -> Result<Simulation, SwapRouteError>;
}

impl SwapChain for SolanaRpcClient {
    async fn latest_blockhash(&self) -> Result<(Hash, u64), SwapRouteError> {
        self.get_latest_blockhash_with_commitment(self.commitment())
            .await
            .map_err(SwapRouteError::solana)
    }

    async fn accounts(
        &self,
        keys: &[Pubkey],
        min_context_slot: u64,
    ) -> Result<AccountsRead, SwapRouteError> {
        let mut read = AccountsRead {
            slot: min_context_slot,
            accounts: Vec::with_capacity(keys.len()),
        };
        for chunk in keys.chunks(MAX_ACCOUNTS_PER_REQUEST) {
            let config = RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                data_slice: None,
                commitment: Some(self.commitment()),
                min_context_slot: Some(read.slot),
            };
            let response = self
                .get_multiple_accounts_with_config(chunk, config)
                .await
                .map_err(SwapRouteError::solana)?;
            read.slot = read.slot.max(response.context.slot);
            read.accounts.extend(response.value);
        }
        Ok(read)
    }

    async fn simulate(
        &self,
        txn: &VersionedTransaction,
        keys: &[Pubkey],
        min_context_slot: u64,
    ) -> Result<Simulation, SwapRouteError> {
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
            min_context_slot: Some(min_context_slot),
            inner_instructions: true,
        };
        let result = self
            .simulate_transaction_with_config(txn, config)
            .await
            .map_err(SwapRouteError::solana)?
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
            inner_instructions: result.inner_instructions,
        })
    }
}

/// Check `txn` against `request` and `min_out` (the quote's minimum output,
/// from [`check_quote`]), set a fresh blockhash, and check a simulation of
/// the unsigned transaction. Returns that unsigned transaction, the one the
/// checks passed, and its last valid block height. Nothing here signs.
///
/// Each chain read asks the node for the slot of the read before it or a
/// later one (`minContextSlot`), so on a node that honours it the simulation
/// runs against state no older than the before-state it is compared with.
/// The answering slot is not checked. It may run against newer state: a
/// change to a taker account that lands between the two reads is counted as
/// the swap's.
pub(crate) async fn verify_unsigned<S: SwapChain>(
    chain: &S,
    request: &SwapTerms,
    min_out: u64,
    txn: VersionedTransaction,
) -> Result<(VersionedTransaction, u64), SwapRouteError> {
    check_signer(&txn.message, &request.taker)?;

    let table_keys: Vec<Pubkey> = txn
        .message
        .address_table_lookups()
        .unwrap_or_default()
        .iter()
        .map(|lookup| lookup.account_key)
        .collect();
    let tables = if table_keys.is_empty() {
        AccountsRead {
            slot: 0,
            accounts: Vec::new(),
        }
    } else {
        chain.accounts(&table_keys, 0).await?
    };
    let resolved = resolve_accounts(&txn.message, &tables.accounts)?;
    let fee_bound = fee_bound(&txn.message, &resolved.keys)?;
    check_route_minimum(&txn.message, &resolved.keys, request.min_output)?;

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

    check_size(&unsigned)?;

    let AccountsRead {
        slot,
        accounts: pre,
    } = chain.accounts(&resolved.writable, tables.slot).await?;
    let simulation = chain.simulate(&unsigned, &resolved.writable, slot).await?;
    if let Some(err) = simulation.err {
        return Err(SwapRefusal::SimulationFailed(err).into());
    }
    let expected = ExpectedEffects {
        request: *request,
        min_out,
        fee_bound,
    };
    check_effects(&expected, &resolved.writable, &pre, &simulation.accounts)?;
    let inner = simulation
        .inner_instructions
        .ok_or(SwapRefusal::SimulationIncomplete)?;
    check_hotspot_programs(&unsigned.message, &resolved.keys, &inner, &request.taker)?;

    Ok((unsigned, last_valid_block_height))
}

/// Refuse a transaction whose serialized length is over [`PACKET_DATA_SIZE`].
/// The RPC node refuses to simulate one that large, with an error that reads
/// as a failed request, so the size is judged here, where it names the
/// cause.
pub(crate) fn check_size(txn: &VersionedTransaction) -> Result<(), SwapRouteError> {
    let size = bincode::serialized_size(txn)
        .map_err(|err| SwapRouteError::TransactionDecode(err.into()))?;
    let limit = PACKET_DATA_SIZE as u64;
    if size > limit {
        return Err(SwapRefusal::TransactionTooLarge { size, limit }.into());
    }
    Ok(())
}

/// Check the quote's terms against the request. Returns the quote's minimum
/// output (`otherAmountThreshold`), which the simulation must meet; it is at
/// least `request.min_output`, so the simulation meets that too.
pub(crate) fn check_quote(quote: &SwapQuote, request: &SwapTerms) -> Result<u64, SwapRefusal> {
    fn same(field: &'static str, expected: String, actual: &str) -> Result<(), SwapRefusal> {
        if actual == expected {
            Ok(())
        } else {
            Err(SwapRefusal::QuoteMismatch {
                field,
                expected,
                actual: bounded_text(actual, MAX_ERROR_BODY_LEN),
            })
        }
    }
    fn amount(field: &'static str, value: &str) -> Result<u64, SwapRefusal> {
        value
            .parse()
            .map_err(|_| SwapRefusal::QuoteMalformed { field })
    }

    same(
        "inputMint",
        request.input_mint.to_string(),
        &quote.input_mint,
    )?;
    same(
        "outputMint",
        request.output_mint.to_string(),
        &quote.output_mint,
    )?;
    same("inAmount", request.amount.to_string(), &quote.in_amount)?;
    same(
        "slippageBps",
        request.slippage_bps.to_string(),
        &quote.slippage_bps.to_string(),
    )?;
    same("swapMode", "ExactIn".to_string(), &quote.swap_mode)?;

    let out_amount = amount("outAmount", &quote.out_amount)?;
    if out_amount == 0 {
        return Err(SwapRefusal::OutAmountZero);
    }
    let threshold = amount("otherAmountThreshold", &quote.other_amount_threshold)?;
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

/// What is left of `amount` once `bps` basis points of slippage are taken
/// off it, rounded down: the least output a slippage allows on a quote.
fn min_out(amount: u64, bps: u16) -> u64 {
    let kept = 10_000u128.saturating_sub(u128::from(bps));
    (u128::from(amount) * kept / 10_000) as u64
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

/// The most SOL the transaction's fee can take from the taker, which pays
/// it as the only signer: the base fee for each signature plus the priority
/// fee at the message's compute unit price and limit. A message that does not
/// set exactly one limit is refused, so the bound is what the runtime charges
/// for that limit; the limit is clamped to [`MAX_COMPUTE_UNIT_LIMIT`], as the
/// runtime clamps it. A price above [`MAX_COMPUTE_UNIT_PRICE`] is refused,
/// since the bound is SOL the transaction may take. Signatures of
/// precompile programs (Ed25519, Secp256k1) are not counted: the runtime
/// charges for them, so a transaction carrying one can spend more than the
/// bound and be refused by [`check_effects`], never accepted past it.
///
/// The limit and price are read by [`compute_budget_settings`], which also
/// serves [`crate::verify::assert_priority_fee_within`].
pub(crate) fn fee_bound(message: &VersionedMessage, keys: &[Pubkey]) -> Result<u64, SwapRefusal> {
    if let Some(index) = message
        .instructions()
        .iter()
        .position(|ix| keys.get(usize::from(ix.program_id_index)).is_none())
    {
        return Err(SwapRefusal::AccountIndexOutOfRange { index });
    }
    let settings = compute_budget_settings(keys, message.instructions());
    let limit = match settings.limit {
        Some(limit) if settings.limit_count == 1 => limit,
        _ => {
            return Err(SwapRefusal::ComputeUnitLimitCount {
                count: settings.limit_count,
            })
        }
    };
    if settings.price > MAX_COMPUTE_UNIT_PRICE {
        return Err(SwapRefusal::ComputeUnitPriceAboveCap {
            price: settings.price,
            cap: MAX_COMPUTE_UNIT_PRICE,
        });
    }
    let budget = ComputeBudget {
        limit: limit.min(MAX_COMPUTE_UNIT_LIMIT),
        price: settings.price,
    };
    Ok(budget.fee_bound(message.header().num_required_signatures))
}

/// Refuse a transaction with a top-level Jupiter route whose own minimum
/// output is below `min_output`, the caller's floor. A route's minimum is the
/// quote it carries less its slippage, so a builder that quotes the caller
/// honestly but sets the route's slippage wide would let the landed swap
/// deliver less than the caller asked for, which the simulation cannot show.
///
/// Only a top-level instruction of the Jupiter aggregator whose data starts
/// with the `route_v2` or `shared_accounts_route_v2` discriminator and holds
/// the arguments that decide the minimum is read. Every other instruction,
/// other data, a recognised discriminator with too few bytes, and Jupiter
/// reached by a call from another program, pass.
pub(crate) fn check_route_minimum(
    message: &VersionedMessage,
    keys: &[Pubkey],
    min_output: u64,
) -> Result<(), SwapRefusal> {
    for (index, ix) in message.instructions().iter().enumerate() {
        if keys.get(usize::from(ix.program_id_index)) != Some(&JUPITER_AGGREGATOR_V6) {
            continue;
        }
        let Some((discriminator, rest)) = ix.data.split_at_checked(ROUTE_V2.len()) else {
            continue;
        };
        let args = if discriminator == ROUTE_V2 {
            rest
        } else if discriminator == SHARED_ACCOUNTS_ROUTE_V2 {
            // The route's `id: u8` comes first.
            rest.get(1..).unwrap_or_default()
        } else {
            continue;
        };
        let Some(terms) = RouteTerms::read(args) else {
            continue;
        };
        // The route's quoted output less its slippage, rounded down: the
        // least the program can enforce from these terms whichever way it
        // rounds. The quote's own threshold can sit one unit above it, as the
        // quote rounds up, so the route is held to the caller's minimum, not
        // to the quote's.
        //
        // The program's IDL does not say whether the program compares that
        // minimum with the output before or after the platform fee is taken
        // from it, so the minimum is assumed to bind the output before the
        // fee, and the fee is taken off it too, rounded down. A route whose
        // minimum meets the caller's only before the fee is refused.
        let route_minimum = min_out(
            min_out(terms.quoted_out_amount, terms.slippage_bps),
            terms.platform_fee_bps,
        );
        if route_minimum < min_output {
            return Err(SwapRefusal::RouteMinimumBelowRequest {
                index,
                route_minimum,
                min_output,
            });
        }
    }
    Ok(())
}

/// The arguments of a Jupiter v2 route that decide its minimum output, in the
/// IDL's order after `in_amount`.
struct RouteTerms {
    quoted_out_amount: u64,
    slippage_bps: u16,
    platform_fee_bps: u16,
}

impl RouteTerms {
    /// Read the terms from `args`, which begin at `in_amount`.
    fn read(args: &[u8]) -> Option<Self> {
        Some(Self {
            quoted_out_amount: u64::from_le_bytes(args.get(8..16)?.try_into().ok()?),
            slippage_bps: u16::from_le_bytes(args.get(16..18)?.try_into().ok()?),
            platform_fee_bps: u16::from_le_bytes(args.get(18..20)?.try_into().ok()?),
        })
    }
}

/// What the simulation must show.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExpectedEffects {
    pub request: SwapTerms,
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
/// simulated swap against what `expected` allows. Only the taker's own
/// account and the token accounts the taker owns count; where the output
/// lands is judged by the taker's balance of the output mint, and any fee a
/// route takes by the output that is left.
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
                if after.delegated_amount != before.delegated_amount || after.state != before.state
                {
                    return Err(SwapRefusal::TokenDelegationChanged { account: *address });
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

/// Refuse a transaction in which any instruction of the Bubblegum program
/// or the lazy distributor program, at the top level or among `inner` (the
/// simulation's inner instructions) at any depth, names `taker`. A top-level
/// instruction names the taker when one of its accounts, resolved through
/// `keys`, is the taker; an inner one when its accounts list the taker or,
/// for an instruction the node parsed, when the taker's address appears
/// anywhere in the parsed form. A top-level instruction whose program index
/// lies outside `keys` is refused, as is an inner one in compiled form whose
/// program or account index does.
pub(crate) fn check_hotspot_programs(
    message: &VersionedMessage,
    keys: &[Pubkey],
    inner: &[UiInnerInstructions],
    taker: &Pubkey,
) -> Result<(), SwapRefusal> {
    let guarded = [programs::bubblegum::ID, programs::lazy_distributor::ID];
    let refused = |program: Pubkey| SwapRefusal::HotspotProgramInstruction { program };
    let key = |index: &u8| keys.get(usize::from(*index));
    for (index, ix) in message.instructions().iter().enumerate() {
        let program =
            key(&ix.program_id_index).ok_or(SwapRefusal::AccountIndexOutOfRange { index })?;
        if guarded.contains(program)
            && ix
                .accounts
                .iter()
                .any(|account| key(account) == Some(taker))
        {
            return Err(refused(*program));
        }
    }

    let taker_text = taker.to_string();
    let guarded_text = |program_id: &str| {
        guarded
            .iter()
            .find(|program| program.to_string() == program_id)
            .copied()
    };
    for ix in inner.iter().flat_map(|group| &group.instructions) {
        let hit = match ix {
            UiInstruction::Compiled(ix) => {
                let program =
                    key(&ix.program_id_index).ok_or(SwapRefusal::InnerInstructionUnreadable)?;
                let accounts = ix
                    .accounts
                    .iter()
                    .map(|account| key(account).ok_or(SwapRefusal::InnerInstructionUnreadable))
                    .collect::<Result<Vec<_>, _>>()?;
                (guarded.contains(program) && accounts.contains(&taker)).then_some(*program)
            }
            UiInstruction::Parsed(UiParsedInstruction::PartiallyDecoded(ix)) => {
                guarded_text(&ix.program_id).filter(|_| ix.accounts.contains(&taker_text))
            }
            UiInstruction::Parsed(UiParsedInstruction::Parsed(ix)) => {
                guarded_text(&ix.program_id).filter(|_| names(&ix.parsed, &taker_text))
            }
        };
        if let Some(program) = hit {
            return Err(refused(program));
        }
    }
    Ok(())
}

/// Whether `value` holds the string `text` anywhere within it.
fn names(value: &Value, text: &str) -> bool {
    match value {
        Value::String(value) => value == text,
        Value::Array(items) => items.iter().any(|item| names(item, text)),
        Value::Object(fields) => fields.values().any(|field| names(field, text)),
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::solana_client::{
        client_error,
        rpc_client::RpcClientConfig,
        rpc_request::{RpcError, RpcRequest},
        rpc_sender::{RpcSender, RpcTransportStats},
    };
    use crate::solana_sdk::{
        instruction::{AccountMeta, Instruction},
        message::Message,
        program_option::COption,
        signature::NullSigner,
        signer::Signer,
    };
    use crate::verify::COMPUTE_UNIT_PRICE_TAG;
    use anchor_spl::token::spl_token::state::AccountState;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use serde::Deserialize;
    use serde_json::{json, Value};
    use solana_address_lookup_table_interface::state::LookupTableMeta;
    use std::sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::{borrow::Cow, collections::BTreeMap, str::FromStr};

    const HNT: Pubkey = pubkey!("hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux");
    const USDC: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    const MOBILE: Pubkey = pubkey!("mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6");
    /// The SPL Memo program, a program no swap invokes.
    const MEMO: Pubkey = pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

    // Swaps recorded from Jupiter's v2 `/order` endpoint (router `metis`)
    // for two public wallets, with the lookup-table entries their
    // transactions reference and the taker's accounts before and after a
    // mainnet simulation of each transaction (`innerInstructions: true`). An
    // order carries every field of the blockchain-api's quote, and its
    // transaction is of the `route_v2` family that service builds for a V2
    // quote, so a fixture is read as a quote and the transaction built for
    // it. The HNT fixture's accounts and inner instructions are from one
    // successful simulation; the SOL fixture's inner instruction list is from
    // a later simulation of the same transaction than its accounts.
    pub(crate) const HNT_USDC: &str = include_str!("../../fixtures/jupiter/hnt_usdc.json");
    pub(crate) const SOL_USDC: &str = include_str!("../../fixtures/jupiter/sol_usdc.json");

    #[derive(Deserialize)]
    struct Fixture {
        order: SwapQuote,
        lookup_tables: HashMap<String, BTreeMap<String, String>>,
        accounts: Vec<FixtureAccount>,
        inner_instructions: Vec<UiInnerInstructions>,
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
    /// included), `post` answers the simulation. Each account read answers
    /// from a slot one past the last, starting after [`FIRST_SLOT`], and
    /// every read records the `min_context_slot` it was given.
    pub(crate) struct FakeChain {
        pub pre: HashMap<Pubkey, Account>,
        pub post: HashMap<Pubkey, Account>,
        pub sim_err: Option<TransactionError>,
        pub drop_post: bool,
        pub inner: Option<Vec<UiInnerInstructions>>,
        pub slot: AtomicU64,
        pub reads: Mutex<Vec<Read>>,
    }

    pub(crate) const FIRST_SLOT: u64 = 1_000;

    /// A chain read: the slot it answered from (none for a simulation) and
    /// the `min_context_slot` it was given.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Read {
        pub answered: Option<u64>,
        pub min_context_slot: u64,
    }

    pub(crate) const BLOCKHASH: Hash = Hash::new_from_array([7; 32]);
    pub(crate) const LAST_VALID_BLOCK_HEIGHT: u64 = 42;

    impl SwapChain for FakeChain {
        async fn latest_blockhash(&self) -> Result<(Hash, u64), SwapRouteError> {
            Ok((BLOCKHASH, LAST_VALID_BLOCK_HEIGHT))
        }

        async fn accounts(
            &self,
            keys: &[Pubkey],
            min_context_slot: u64,
        ) -> Result<AccountsRead, SwapRouteError> {
            let slot = self.slot.fetch_add(1, Ordering::SeqCst) + 1;
            self.reads.lock().expect("lock").push(Read {
                answered: Some(slot),
                min_context_slot,
            });
            Ok(AccountsRead {
                slot,
                accounts: keys.iter().map(|key| self.pre.get(key).cloned()).collect(),
            })
        }

        async fn simulate(
            &self,
            txn: &VersionedTransaction,
            keys: &[Pubkey],
            min_context_slot: u64,
        ) -> Result<Simulation, SwapRouteError> {
            self.reads.lock().expect("lock").push(Read {
                answered: None,
                min_context_slot,
            });
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
                inner_instructions: self.inner.clone(),
            })
        }
    }

    pub(crate) struct Case {
        pub quote: SwapQuote,
        pub transaction: VersionedTransaction,
        pub request: SwapTerms,
        pub chain: FakeChain,
    }

    pub(crate) fn case(fixture: &str) -> Case {
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
        let mut quote = fixture.order;
        let mut order_field = |field: &str| match &mut quote.extra {
            Value::Object(fields) => match fields.remove(field) {
                Some(Value::String(value)) => value,
                other => panic!("fixture {field} is {other:?}"),
            },
            other => panic!("fixture order is {other:?}"),
        };
        let transaction = order_field("transaction");
        let taker = key(&order_field("taker"));
        let transaction =
            bincode::deserialize(&BASE64.decode(transaction).expect("decode fixture"))
                .expect("deserialize fixture");
        let threshold: u64 = quote
            .other_amount_threshold
            .parse()
            .expect("fixture otherAmountThreshold");
        let request = SwapTerms {
            input_mint: key(&quote.input_mint),
            output_mint: key(&quote.output_mint),
            amount: quote.in_amount.parse().expect("fixture inAmount"),
            // The most the recorded route still guarantees once its platform
            // fee is taken off the quote's minimum, which must pass.
            min_output: min_out(threshold, recorded_route_fee_bps(&transaction)),
            slippage_bps: quote.slippage_bps.try_into().expect("fixture slippageBps"),
            taker,
        };
        Case {
            quote,
            transaction,
            request,
            chain: FakeChain {
                pre,
                post,
                sim_err: None,
                drop_post: false,
                inner: Some(fixture.inner_instructions),
                slot: AtomicU64::new(FIRST_SLOT),
                reads: Mutex::default(),
            },
        }
    }

    /// The platform fee, in basis points, of the one Jupiter route
    /// instruction in a recorded transaction.
    fn recorded_route_fee_bps(transaction: &VersionedTransaction) -> u16 {
        let data = transaction
            .message
            .instructions()
            .iter()
            .map(|ix| ix.data.as_slice())
            .find(|data| data.starts_with(&ROUTE_V2) || data.starts_with(&SHARED_ACCOUNTS_ROUTE_V2))
            .expect("fixture route instruction");
        let args = if data.starts_with(&ROUTE_V2) {
            &data[ROUTE_V2.len()..]
        } else {
            &data[SHARED_ACCOUNTS_ROUTE_V2.len() + 1..]
        };
        RouteTerms::read(args)
            .expect("fixture route terms")
            .platform_fee_bps
    }

    /// A signer that counts the messages it signs.
    pub(crate) struct CountingSigner {
        inner: NullSigner,
        signed: AtomicUsize,
    }

    impl CountingSigner {
        pub fn new(pubkey: &Pubkey) -> Self {
            Self {
                inner: NullSigner::new(pubkey),
                signed: AtomicUsize::new(0),
            }
        }

        pub fn signed(&self) -> usize {
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
        async fn run(&self) -> Result<(VersionedTransaction, u64), SwapRouteError> {
            let signer = NullSigner::new(&self.request.taker);
            self.run_with(&signer).await
        }

        async fn run_with(
            &self,
            signer: &(dyn Signer + Sync),
        ) -> Result<(VersionedTransaction, u64), SwapRouteError> {
            let min_out = check_quote(&self.quote, &self.request)?;
            let (unsigned, last_valid_block_height) = verify_unsigned(
                &self.chain,
                &self.request,
                min_out,
                self.transaction.clone(),
            )
            .await?;
            let txn = VersionedTransaction::try_new(unsigned.message, &[signer])
                .map_err(SwapRouteError::Signing)?;
            Ok((txn, last_valid_block_height))
        }

        /// Run with a counting signer and return the result with the number
        /// of messages signed.
        async fn run_counting(
            &self,
        ) -> (Result<(VersionedTransaction, u64), SwapRouteError>, usize) {
            let signer = CountingSigner::new(&self.request.taker);
            let result = self.run_with(&signer).await;
            (result, signer.signed())
        }

        async fn refusal(&self) -> SwapRefusal {
            match self.run().await {
                Err(SwapRouteError::Refused(refusal)) => refusal,
                other => panic!("expected a refusal, got {other:?}"),
            }
        }

        pub fn message(&self) -> VersionedMessage {
            self.transaction.message.clone()
        }

        pub fn set_message(&mut self, message: VersionedMessage) {
            self.transaction = VersionedTransaction {
                signatures: vec![Default::default()],
                message,
            };
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
            fee_bound(&self.message(), &self.resolved().keys).expect("fixture fee bound")
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
        let (txn, last_valid_block_height) = result.expect("signed");
        assert_eq!(signed, 1);
        assert_eq!(*txn.message.recent_blockhash(), BLOCKHASH);
        assert_eq!(last_valid_block_height, LAST_VALID_BLOCK_HEIGHT);
        assert_eq!(txn.signatures.len(), 1);
    }

    #[tokio::test]
    async fn recorded_sol_to_usdc_route_passing_the_wallet_to_amms_is_signed() {
        // This `route_v2` hands the taker to Tessera, Raydium CLMM, Manifest
        // and AlphaQ swap instructions, each of which runs taker-signed token
        // transfers beneath it.
        let case = case(SOL_USDC);
        let (result, signed) = case.run_counting().await;
        let (txn, _) = result.expect("signed");
        assert_eq!(signed, 1);
        assert_eq!(txn.signatures.len(), 1);
    }

    #[test]
    fn recorded_transactions_spend_exactly_their_fee_bound() {
        // The simulations charged exactly the fee each message declares,
        // so the recorded swaps sit on the lamport bound.
        let hnt = case(HNT_USDC);
        assert_eq!(hnt.fee_bound(), 5_000 + 1_119);
        let sol = case(SOL_USDC);
        assert_eq!(sol.fee_bound(), 5_000 + 54_926);
    }

    // ---- quote ----

    #[tokio::test]
    async fn quote_for_another_amount_is_refused() {
        let mut case = case(HNT_USDC);
        case.request.amount += 1;
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::QuoteMismatch {
                field: "inAmount",
                ..
            }
        ));
    }

    pub(crate) fn quote_field(edit: impl FnOnce(&mut SwapQuote, &mut SwapTerms)) -> SwapRefusal {
        let Case {
            mut quote,
            mut request,
            ..
        } = case(HNT_USDC);
        assert!(check_quote(&quote, &request).is_ok());
        edit(&mut quote, &mut request);
        check_quote(&quote, &request).expect_err("refused")
    }

    #[test]
    fn quote_terms_must_match_the_request() {
        let mismatch = |refusal: SwapRefusal| match refusal {
            SwapRefusal::QuoteMismatch { field, .. } => field,
            other => panic!("expected QuoteMismatch, got {other:?}"),
        };
        assert_eq!(
            mismatch(quote_field(|quote, _| quote.input_mint = MOBILE.to_string())),
            "inputMint"
        );
        assert_eq!(
            mismatch(quote_field(
                |quote, _| quote.output_mint = MOBILE.to_string()
            )),
            "outputMint"
        );
        assert_eq!(
            mismatch(quote_field(
                |quote, _| quote.in_amount = "99999999".to_string()
            )),
            "inAmount"
        );
        assert_eq!(
            mismatch(quote_field(|_, request| request.slippage_bps = 50)),
            "slippageBps"
        );
        assert_eq!(
            mismatch(quote_field(|quote, _| quote.slippage_bps = 50)),
            "slippageBps"
        );
        assert_eq!(
            mismatch(quote_field(
                |quote, _| quote.swap_mode = "ExactOut".to_string()
            )),
            "swapMode"
        );
    }

    #[test]
    fn quote_threshold_must_honour_the_slippage() {
        // 360711 at 100 bps rounds down to 357103. The caller's own minimum
        // sits below it, so only the slippage refuses.
        let Case {
            mut quote,
            mut request,
            ..
        } = case(HNT_USDC);
        request.min_output = 1;
        assert_eq!(quote.other_amount_threshold, "357103");
        assert_eq!(
            check_quote(&quote, &request).expect("at the floor"),
            357_103
        );
        quote.other_amount_threshold = "357102".to_string();
        assert!(matches!(
            check_quote(&quote, &request),
            Err(SwapRefusal::ThresholdBelowSlippage {
                min_out: 357_103,
                ..
            })
        ));
        quote.other_amount_threshold = "-1".to_string();
        assert!(matches!(
            check_quote(&quote, &request),
            Err(SwapRefusal::QuoteMalformed {
                field: "otherAmountThreshold"
            })
        ));
        assert!(matches!(
            quote_field(|quote, _| quote.out_amount = "lots".to_string()),
            SwapRefusal::QuoteMalformed { field: "outAmount" }
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

    // ---- fee bound ----

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
            SwapRefusal::ComputeUnitPriceAboveCap {
                price: 2_500_001,
                cap: 2_500_000
            }
        ));
    }

    /// The fee bound of a legacy message for `taker` holding
    /// `instructions`.
    fn fee_bound_of(taker: &Pubkey, instructions: &[Instruction]) -> Result<u64, SwapRefusal> {
        let message = VersionedMessage::Legacy(Message::new(instructions, Some(taker)));
        fee_bound(&message, message.static_account_keys())
    }

    #[test]
    fn fee_bound_prices_the_limit_the_runtime_charges_for() {
        use solana_compute_budget_interface::ComputeBudgetInstruction;
        let taker = Pubkey::new_unique();
        let limit = ComputeBudgetInstruction::set_compute_unit_limit;
        let price = ComputeBudgetInstruction::set_compute_unit_price;
        // 1,000 µL × 300,000 CU = 300 lamports.
        assert_eq!(
            fee_bound_of(&taker, &[limit(300_000), price(1_000)]).ok(),
            Some(5_300)
        );
        // A limit past the runtime's maximum is clamped to it.
        assert_eq!(
            fee_bound_of(&taker, &[limit(5_000_000), price(1_000)]).ok(),
            Some(6_400)
        );
        // A repeated price is priced at the highest, in either order.
        for prices in [[price(2_000), price(1_000)], [price(1_000), price(2_000)]] {
            assert_eq!(
                fee_bound_of(&taker, &[vec![limit(300_000)], prices.to_vec()].concat()).ok(),
                Some(5_600)
            );
        }
        // No price: the base fee alone.
        assert_eq!(fee_bound_of(&taker, &[limit(300_000)]).ok(), Some(5_000));
        // The price cap: 2,500,000 µL × 1,400,000 CU = 3,500,000 lamports.
        assert_eq!(
            fee_bound_of(&taker, &[limit(1_400_000), price(2_500_000)]).ok(),
            Some(3_505_000)
        );
        assert!(matches!(
            fee_bound_of(&taker, &[limit(1_400_000), price(2_500_001)]),
            Err(SwapRefusal::ComputeUnitPriceAboveCap {
                price: 2_500_001,
                cap: 2_500_000
            })
        ));
    }

    #[test]
    fn exactly_one_compute_unit_limit_is_required() {
        use solana_compute_budget_interface::ComputeBudgetInstruction;
        let taker = Pubkey::new_unique();
        let limit = ComputeBudgetInstruction::set_compute_unit_limit;
        let price = ComputeBudgetInstruction::set_compute_unit_price;
        let memo = Instruction::new_with_bytes(MEMO, &[], vec![]);
        assert!(matches!(
            fee_bound_of(&taker, &[price(1_000), memo.clone()]),
            Err(SwapRefusal::ComputeUnitLimitCount { count: 0 })
        ));
        // The higher limit is not the last, so neither ordering reads one.
        for limits in [
            [limit(300_000), limit(100_000)],
            [limit(100_000), limit(300_000)],
        ] {
            assert!(matches!(
                fee_bound_of(&taker, &[vec![price(1_000)], limits.to_vec()].concat()),
                Err(SwapRefusal::ComputeUnitLimitCount { count: 2 })
            ));
        }
        fee_bound_of(&taker, &[limit(300_000), memo]).expect("one limit");
    }

    #[test]
    fn fee_payer_must_be_the_taker() {
        let taker = Pubkey::new_unique();
        check_signer(&legacy_message(1, vec![taker]), &taker).expect("the taker pays");
        // One signer, which is not the taker.
        let message = legacy_message(1, vec![Pubkey::new_unique(), taker]);
        assert!(matches!(
            check_signer(&message, &taker),
            Err(SwapRefusal::FeePayerNotTaker)
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
        let threshold = 357_103;
        let floor = case.token_amount(&case.chain.pre, &usdc) + threshold;
        case.edit_post_token(&usdc, |token| token.amount = floor);
        case.run().await.expect("exactly the threshold passes");
        case.edit_post_token(&usdc, |token| token.amount = floor - 1);
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::OutputBelowThreshold {
                threshold: 357_103,
                received: 357_102
            }
        ));
    }

    #[tokio::test]
    async fn input_spent_other_than_the_amount_is_refused() {
        // One unit more of HNT leaves, then one unit less.
        for (change, spent) in [(-1i64, 100_000_001), (1, 99_999_999)] {
            let mut case = case(HNT_USDC);
            let hnt = case.token_account_of(&HNT);
            let post = case.token_amount(&case.chain.post, &hnt);
            case.edit_post_token(&hnt, |token| {
                token.amount = post.checked_add_signed(change).expect("amount")
            });
            let refusal = case.refusal().await;
            assert!(
                matches!(
                    refusal,
                    SwapRefusal::InputSpent {
                        expected: 100_000_000,
                        actual,
                    } if actual == spent
                ),
                "{refusal:?}"
            );
        }
    }

    #[tokio::test]
    async fn lamports_beyond_the_fee_bound_are_refused() {
        // The fee bound, plus the input amount when the input is SOL; each
        // recorded swap spends exactly that, so one lamport more is refused.
        for (fixture, bound) in [(HNT_USDC, 6_119), (SOL_USDC, 10_059_926)] {
            let mut case = case(fixture);
            let taker = case.request.taker;
            case.chain.post.get_mut(&taker).expect("taker").lamports -= 1;
            let refusal = case.refusal().await;
            assert!(
                matches!(
                    refusal,
                    SwapRefusal::LamportsOverBound { spent, bound: refused }
                        if refused == bound && spent == bound + 1
                ),
                "{refusal:?}"
            );
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
        let (result, signed) = case.run_counting().await;
        assert!(
            matches!(
                result,
                Err(SwapRouteError::Refused(SwapRefusal::SignerCount {
                    num_signers: 2,
                    ..
                }))
            ),
            "{result:?}"
        );
        assert_eq!(signed, 0);
    }

    /// The size `check_size` judges: the serialized length of `case`'s
    /// transaction once its signature slots are empty, as `verify_unsigned`
    /// builds it.
    fn unsigned_size(case: &Case) -> u64 {
        let message = case.message();
        let unsigned = VersionedTransaction {
            signatures: vec![
                Signature::default();
                usize::from(message.header().num_required_signatures)
            ],
            message,
        };
        bincode::serialized_size(&unsigned).expect("size the transaction")
    }

    /// Pad the data of `case`'s longest instruction until its unsigned
    /// transaction is exactly `target` bytes, changing nothing else. The
    /// data's length prefix grows by a byte when the data reaches 128 bytes,
    /// so one byte fewer of padding is tried when the first overshoots.
    fn pad_to(case: &mut Case, target: u64) {
        let base = case.message();
        let need = target
            .checked_sub(unsigned_size(case))
            .expect("fixture fits the target") as usize;
        for pad in [need, need.saturating_sub(1)] {
            let mut message = base.clone();
            let VersionedMessage::V0(v0) = &mut message else {
                panic!("fixture is v0");
            };
            let data = &mut v0
                .instructions
                .iter_mut()
                .max_by_key(|ix| ix.data.len())
                .expect("an instruction")
                .data;
            data.resize(data.len() + pad, 0);
            case.set_message(message);
            if unsigned_size(case) == target {
                return;
            }
        }
        panic!("no padding reaches {target} bytes");
    }

    #[tokio::test]
    async fn transaction_at_the_packet_limit_is_signed_and_one_byte_over_is_refused() {
        let limit = PACKET_DATA_SIZE as u64;
        let mut case = case(HNT_USDC);
        pad_to(&mut case, limit);
        case.run()
            .await
            .expect("a transaction at the limit is signed");

        pad_to(&mut case, limit + 1);
        match case.refusal().await {
            SwapRefusal::TransactionTooLarge { size, limit: shown } => {
                assert_eq!((size, shown), (limit + 1, limit));
            }
            other => panic!("expected TransactionTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn oversize_transaction_is_refused_before_it_is_simulated() {
        let mut case = case(HNT_USDC);
        pad_to(&mut case, PACKET_DATA_SIZE as u64 + 1);
        let refusal = case.refusal().await;
        assert!(
            matches!(refusal, SwapRefusal::TransactionTooLarge { .. }),
            "{refusal:?}"
        );
        let reads = case.chain.reads.lock().expect("lock").clone();
        assert!(
            reads.iter().all(|read| read.answered.is_some()),
            "{reads:?}"
        );
    }

    #[test]
    fn transaction_too_large_names_its_size_and_the_limit() {
        let refusal = SwapRefusal::TransactionTooLarge {
            size: 1266,
            limit: 1232,
        };
        assert_eq!(
            refusal.to_string(),
            "swap refused: the transaction is 1266 bytes, over the 1232 byte limit"
        );
    }

    #[tokio::test]
    async fn each_read_is_no_older_than_the_one_before() {
        let case = case(HNT_USDC);
        case.run().await.expect("signed");
        let reads = case.chain.reads.lock().expect("lock").clone();
        // The lookup tables, the before-state, then the simulation.
        assert_eq!(
            reads,
            vec![
                Read {
                    answered: Some(FIRST_SLOT + 1),
                    min_context_slot: 0,
                },
                Read {
                    answered: Some(FIRST_SLOT + 2),
                    min_context_slot: FIRST_SLOT + 1,
                },
                Read {
                    answered: None,
                    min_context_slot: FIRST_SLOT + 2,
                },
            ]
        );
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

    #[tokio::test]
    async fn recorded_route_passing_the_wallet_to_amms_still_refuses_another_token_loss() {
        let mut case = case(SOL_USDC);
        let address = untouched_writable(&case);
        let taker = case.request.taker;
        case.chain
            .pre
            .insert(address, token_account_state(MOBILE, taker, 10));
        case.chain
            .post
            .insert(address, token_account_state(MOBILE, taker, 9));
        let (result, signed) = case.run_counting().await;
        assert!(
            matches!(
                &result,
                Err(SwapRouteError::Refused(SwapRefusal::OtherTokenDecreased { mint }))
                    if *mint == pubkey!("mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6")
            ),
            "{result:?}"
        );
        assert_eq!(signed, 0);
    }

    #[test]
    fn sol_output_must_reach_the_threshold_net_of_fees() {
        let taker = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let request = SwapTerms {
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
    fn quote_minimum_must_reach_the_callers_minimum() {
        let Case {
            quote, mut request, ..
        } = case(HNT_USDC);
        request.min_output = 357_103;
        assert_eq!(
            check_quote(&quote, &request).expect("at the caller's minimum"),
            357_103
        );
        request.min_output = 357_104;
        assert!(matches!(
            check_quote(&quote, &request),
            Err(SwapRefusal::ThresholdBelowMinimum {
                threshold: 357_103,
                min_output: 357_104
            })
        ));
    }

    #[tokio::test]
    async fn zero_quote_is_refused_unsigned() {
        let mut case = case(HNT_USDC);
        case.quote.out_amount = "0".to_string();
        case.quote.other_amount_threshold = "0".to_string();
        let (result, signed) = case.run_counting().await;
        assert!(
            matches!(
                result,
                Err(SwapRouteError::Refused(SwapRefusal::OutAmountZero))
            ),
            "{result:?}"
        );
        assert_eq!(signed, 0);
    }

    #[tokio::test]
    async fn quote_whose_minimum_is_zero_is_refused() {
        // A 100% slippage quote: a real output, a zero minimum.
        let mut case = case(HNT_USDC);
        case.quote.slippage_bps = 10_000;
        case.request.slippage_bps = 10_000;
        case.quote.other_amount_threshold = "0".to_string();
        assert!(matches!(
            case.refusal().await,
            SwapRefusal::ThresholdBelowMinimum { threshold: 0, .. }
        ));
    }

    #[test]
    fn quote_mismatch_text_is_bounded() {
        let refusal =
            quote_field(|quote, _| quote.swap_mode = format!("\u{1b}[2J{}", "x".repeat(1_000)));
        let SwapRefusal::QuoteMismatch { actual, .. } = refusal else {
            panic!("expected QuoteMismatch, got {refusal:?}");
        };
        assert!(!actual.contains('\u{1b}'));
        assert!(actual.starts_with("\\u{1b}[2J"));
        assert!(actual.chars().count() <= MAX_ERROR_BODY_LEN + 8);
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
            Err(SwapRouteError::Refused(
                SwapRefusal::LamportsOverBound { .. }
            ))
        ));
        assert_eq!(signed, 0);

        let mut failed = self::case(SOL_USDC);
        failed.chain.sim_err = Some(TransactionError::InsufficientFundsForFee);
        let (result, signed) = failed.run_counting().await;
        assert!(matches!(
            result,
            Err(SwapRouteError::Refused(SwapRefusal::SimulationFailed(_)))
        ));
        assert_eq!(signed, 0);
    }

    // ---- RPC errors ----

    #[tokio::test]
    async fn rpc_errors_carry_no_url() {
        let rpc = SolanaRpcClient::new("http://127.0.0.1:1/?api-key=SECRET-KEY".to_string());
        let err = SwapChain::latest_blockhash(&rpc)
            .await
            .expect_err("nothing listens on port 1");
        assert!(matches!(err, SwapRouteError::Solana(_)), "{err:?}");
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

    // ---- Bubblegum ----

    /// A partially decoded instruction of `program` listing `accounts`, as
    /// the node returns one it has no parser for.
    fn program_call(program: &Pubkey, accounts: &[Pubkey], height: u32) -> Value {
        json!({
            "accounts": accounts.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "data": "3Rg2bKuTGTyGGVTrjBjM",
            "programId": program.to_string(),
            "stackHeight": height,
        })
    }

    /// The SOL fixture's inner instructions with `extra` inserted beneath
    /// Tessera's swap, the first instruction of the route's group, which the
    /// route hands the taker.
    fn beneath_the_first_amm(case: &mut Case, extra: Value) {
        let mut inner = serde_json::to_value(case.chain.inner.take().expect("fixture inner"))
            .expect("serialize inner instructions");
        let route = inner[1]["instructions"]
            .as_array_mut()
            .expect("route group instructions");
        assert_eq!(
            route[0]["programId"],
            "TessVdML9pBGgG9yGks7o4HewRaXVAMuoVj4x83GLQH"
        );
        assert_eq!(route[0]["stackHeight"], 2);
        route.insert(1, extra);
        case.chain.inner = Some(serde_json::from_value(inner).expect("inner instructions"));
    }

    #[tokio::test]
    async fn recorded_route_with_a_compressed_nft_transfer_beneath_an_amm_is_refused() {
        // A Bubblegum `transfer` whose leaf owner is the taker would move one
        // of the taker's compressed NFTs, which no token balance shows.
        let mut case = case(SOL_USDC);
        let taker = case.request.taker;
        // tree authority, leaf owner, leaf delegate, new leaf owner, merkle
        // tree, log wrapper, compression program, System.
        let transfer = program_call(
            &pubkey!("BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY"),
            &[
                Pubkey::new_unique(),
                taker,
                taker,
                Pubkey::new_unique(),
                Pubkey::new_unique(),
                pubkey!("noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV"),
                pubkey!("cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK"),
                pubkey!("11111111111111111111111111111111"),
            ],
            3,
        );
        beneath_the_first_amm(&mut case, transfer);
        let (result, signed) = case.run_counting().await;
        assert!(
            matches!(
                result,
                Err(SwapRouteError::Refused(
                    SwapRefusal::HotspotProgramInstruction { .. }
                ))
            ),
            "{result:?}"
        );
        assert_eq!(signed, 0);
    }

    #[tokio::test]
    async fn recorded_route_with_an_unknown_program_given_the_wallet_beneath_an_amm_is_signed() {
        // A program no swap is known to call, handed the taker below the
        // AMM the route invokes, with the recorded effects unchanged.
        let mut case = case(SOL_USDC);
        let taker = case.request.taker;
        beneath_the_first_amm(
            &mut case,
            program_call(&MEMO, &[taker, Pubkey::new_unique()], 3),
        );
        let (result, signed) = case.run_counting().await;
        result.expect("signed");
        assert_eq!(signed, 1);
    }

    /// A legacy message for `taker` with one top-level instruction of
    /// `program` listing `accounts`, and its keys.
    fn top_level(taker: &Pubkey, program: Pubkey, accounts: &[Pubkey]) -> VersionedMessage {
        let metas = accounts
            .iter()
            .map(|account| AccountMeta::new_readonly(*account, false))
            .collect();
        let ix = Instruction::new_with_bytes(program, &[], metas);
        VersionedMessage::Legacy(Message::new(&[ix], Some(taker)))
    }

    fn hotspot_check(
        message: &VersionedMessage,
        inner: Value,
        taker: &Pubkey,
    ) -> Result<(), SwapRefusal> {
        let inner: Vec<UiInnerInstructions> =
            serde_json::from_value(inner).expect("inner instructions");
        check_hotspot_programs(message, message.static_account_keys(), &inner, taker)
    }

    #[test]
    fn a_top_level_bubblegum_instruction_naming_the_wallet_is_refused() {
        let taker = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        let bubblegum = programs::bubblegum::ID;
        hotspot_check(&top_level(&taker, bubblegum, &[other]), json!([]), &taker)
            .expect("a Bubblegum instruction not naming the wallet passes");
        assert!(matches!(
            hotspot_check(
                &top_level(&taker, bubblegum, &[other, taker]),
                json!([]),
                &taker
            ),
            Err(SwapRefusal::HotspotProgramInstruction { .. })
        ));
        hotspot_check(&top_level(&taker, MEMO, &[taker]), json!([]), &taker)
            .expect("another program naming the wallet passes");
    }

    #[test]
    fn an_inner_bubblegum_instruction_naming_the_wallet_is_refused_at_any_depth() {
        let taker = Pubkey::new_unique();
        let bubblegum = programs::bubblegum::ID;
        let message = top_level(&taker, MEMO, &[]);
        let group = |ix: Value| json!([{ "index": 0, "instructions": [ix] }]);
        for height in [2, 3, 5] {
            assert!(
                matches!(
                    hotspot_check(
                        &message,
                        group(program_call(&bubblegum, &[taker], height)),
                        &taker
                    ),
                    Err(SwapRefusal::HotspotProgramInstruction { .. })
                ),
                "height {height}"
            );
        }
        let mut no_height = program_call(&bubblegum, &[taker], 2);
        no_height
            .as_object_mut()
            .expect("instruction object")
            .remove("stackHeight");
        assert!(matches!(
            hotspot_check(&message, group(no_height), &taker),
            Err(SwapRefusal::HotspotProgramInstruction { .. })
        ));
        // A parsed form naming the wallet anywhere within it.
        let parsed = json!({
            "parsed": { "info": { "leafOwner": taker.to_string() }, "type": "transfer" },
            "program": "bubblegum",
            "programId": bubblegum.to_string(),
            "stackHeight": 3,
        });
        assert!(matches!(
            hotspot_check(&message, group(parsed), &taker),
            Err(SwapRefusal::HotspotProgramInstruction { .. })
        ));
        hotspot_check(
            &message,
            group(program_call(&bubblegum, &[Pubkey::new_unique()], 3)),
            &taker,
        )
        .expect("a Bubblegum instruction not naming the wallet passes");
        hotspot_check(&message, group(program_call(&MEMO, &[taker], 4)), &taker)
            .expect("another program naming the wallet passes");
    }

    #[test]
    fn a_compiled_inner_bubblegum_instruction_naming_the_wallet_is_refused() {
        let taker = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        // Keys: the taker, `other`, then Bubblegum.
        let message = top_level(&taker, programs::bubblegum::ID, &[other]);
        let keys = message.static_account_keys();
        assert_eq!(keys, [taker, other, programs::bubblegum::ID]);
        let compiled = |program: u8, account: u8| {
            json!([{ "index": 0, "instructions": [{
                "programIdIndex": program,
                "accounts": [account],
                "data": "3Rg2bKuTGTyGGVTrjBjM",
                "stackHeight": 2,
            }] }])
        };
        hotspot_check(&message, compiled(2, 1), &taker).expect("not naming the wallet");
        assert!(matches!(
            hotspot_check(&message, compiled(2, 0), &taker),
            Err(SwapRefusal::HotspotProgramInstruction { .. })
        ));
        assert!(matches!(
            hotspot_check(&message, compiled(3, 0), &taker),
            Err(SwapRefusal::InnerInstructionUnreadable)
        ));
        // An account index outside the keys is unreadable too, rather than
        // read as not naming the wallet.
        assert!(matches!(
            hotspot_check(&message, compiled(2, 3), &taker),
            Err(SwapRefusal::InnerInstructionUnreadable)
        ));
    }

    #[test]
    fn a_top_level_program_index_outside_the_keys_is_refused() {
        let taker = Pubkey::new_unique();
        let VersionedMessage::Legacy(mut message) = top_level(&taker, MEMO, &[]) else {
            panic!("legacy message");
        };
        message.instructions[0].program_id_index = 9;
        let message = VersionedMessage::Legacy(message);
        assert!(matches!(
            hotspot_check(&message, json!([]), &taker),
            Err(SwapRefusal::AccountIndexOutOfRange { index: 0 })
        ));
        assert!(matches!(
            fee_bound(&message, message.static_account_keys()),
            Err(SwapRefusal::AccountIndexOutOfRange { index: 0 })
        ));
    }

    #[tokio::test]
    async fn a_simulation_without_inner_instructions_is_refused() {
        let mut case = case(HNT_USDC);
        case.chain.inner = None;
        let (result, signed) = case.run_counting().await;
        assert!(matches!(
            result,
            Err(SwapRouteError::Refused(SwapRefusal::SimulationIncomplete))
        ));
        assert_eq!(signed, 0);
    }

    fn legacy_message(num_required_signatures: u8, keys: Vec<Pubkey>) -> VersionedMessage {
        use crate::solana_sdk::message::MessageHeader;
        VersionedMessage::Legacy(Message {
            header: MessageHeader {
                num_required_signatures,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            account_keys: keys,
            recent_blockhash: Hash::default(),
            instructions: vec![],
        })
    }

    #[test]
    fn taker_only_signer_ok() {
        let taker = Pubkey::new_unique();
        let msg = legacy_message(1, vec![taker]);
        check_signer(&msg, &taker).expect("the taker alone signs");
    }

    #[test]
    fn rfq_maker_cosign_rejected() {
        let taker = Pubkey::new_unique();
        let maker = Pubkey::new_unique();
        // RFQ shape: maker is the fee payer (slot 0), taker signs slot 1.
        let msg = legacy_message(2, vec![maker, taker]);
        match check_signer(&msg, &taker) {
            Err(SwapRefusal::SignerCount {
                num_signers,
                extra_signers,
            }) => {
                assert_eq!(num_signers, 2);
                assert_eq!(extra_signers, maker.to_string());
            }
            other => panic!("expected SignerCount, got {other:?}"),
        }
    }

    /// An RPC transport that records each request and answers account reads
    /// from the slots in `slots`, in turn.
    struct RecordingSender {
        slots: Mutex<Vec<u64>>,
        requests: Arc<Mutex<Vec<(String, Value)>>>,
    }

    #[async_trait::async_trait]
    impl RpcSender for RecordingSender {
        async fn send(&self, request: RpcRequest, params: Value) -> client_error::Result<Value> {
            let method = request.to_string();
            self.requests
                .lock()
                .expect("lock")
                .push((method.clone(), params.clone()));
            match method.as_str() {
                "getMultipleAccounts" => {
                    let keys = params[0].as_array().map(Vec::len).unwrap_or_default();
                    let slot = self.slots.lock().expect("lock").remove(0);
                    Ok(json!({ "context": { "slot": slot }, "value": vec![Value::Null; keys] }))
                }
                _ => Err(RpcError::ForUser("not answered".to_string()).into()),
            }
        }

        fn get_transport_stats(&self) -> RpcTransportStats {
            Default::default()
        }

        fn url(&self) -> String {
            "recording".to_string()
        }
    }

    #[tokio::test]
    async fn rpc_reads_ask_for_their_min_context_slot() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let rpc = SolanaRpcClient::new_sender(
            RecordingSender {
                // The second chunk's node answers from an older slot than
                // the first's.
                slots: Mutex::new(vec![12, 9]),
                requests: requests.clone(),
            },
            RpcClientConfig::default(),
        );
        let keys: Vec<Pubkey> = (0..=MAX_ACCOUNTS_PER_REQUEST)
            .map(|_| Pubkey::new_unique())
            .collect();
        let read = SwapChain::accounts(&rpc, &keys, 7).await.expect("read");
        assert_eq!(read.slot, 12);
        assert_eq!(read.accounts.len(), keys.len());
        let case = case(HNT_USDC);
        // The answer is refused; only the request is of interest.
        let _ = SwapChain::simulate(&rpc, &case.transaction, &keys[..1], read.slot).await;
        let requests = requests.lock().expect("lock");
        let min_slots: Vec<_> = requests
            .iter()
            .map(|(method, params)| (method.as_str(), params[1]["minContextSlot"].as_u64()))
            .collect();
        assert_eq!(
            min_slots,
            vec![
                ("getMultipleAccounts", Some(7)),
                ("getMultipleAccounts", Some(12)),
                ("simulateTransaction", Some(12)),
            ]
        );
    }

    #[test]
    fn a_message_requiring_no_signature_is_refused() {
        let taker = Pubkey::new_unique();
        let msg = legacy_message(0, vec![taker]);
        assert!(matches!(
            check_signer(&msg, &taker),
            Err(SwapRefusal::SignerCount { num_signers: 0, .. })
        ));
    }

    // ---- route minimum ----

    /// Arguments from `in_amount` on: `in_amount`, `quoted_out_amount`,
    /// `slippage_bps`, `platform_fee_bps`, `positive_slippage_bps`, and an
    /// empty route plan.
    fn route_args(
        in_amount: u64,
        quoted_out: u64,
        slippage_bps: u16,
        platform_fee_bps: u16,
    ) -> Vec<u8> {
        let mut args = in_amount.to_le_bytes().to_vec();
        args.extend_from_slice(&quoted_out.to_le_bytes());
        args.extend_from_slice(&slippage_bps.to_le_bytes());
        args.extend_from_slice(&platform_fee_bps.to_le_bytes());
        args.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        args
    }

    fn route_v2_data(args: &[u8]) -> Vec<u8> {
        [ROUTE_V2.as_slice(), args].concat()
    }

    /// `shared_accounts_route_v2` data, whose `id: u8` precedes the
    /// arguments.
    fn shared_route_data(args: &[u8]) -> Vec<u8> {
        [SHARED_ACCOUNTS_ROUTE_V2.as_slice(), &[7], args].concat()
    }

    fn route_minimum_check(
        program: Pubkey,
        data: &[u8],
        min_output: u64,
    ) -> Result<(), SwapRefusal> {
        let taker = Pubkey::new_unique();
        let ix = Instruction::new_with_bytes(program, data, vec![]);
        let message = VersionedMessage::Legacy(Message::new(&[ix], Some(&taker)));
        check_route_minimum(&message, message.static_account_keys(), min_output)
    }

    #[test]
    fn jupiter_program_id_and_route_discriminators_are_the_published_ones() {
        assert_eq!(
            JUPITER_AGGREGATOR_V6,
            pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4")
        );
        assert_eq!(hex(&ROUTE_V2), "bb64facc31c4af14");
        assert_eq!(hex(&SHARED_ACCOUNTS_ROUTE_V2), "d19853937cfed8e9");
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn a_route_minimum_below_the_request_is_refused_in_each_form() {
        // 1,000,000 quoted less 100 bps slippage is 990,000, less a 10 bps
        // platform fee is 989,010.
        let args = route_args(5_000_000, 1_000_000, 100, 10);
        for data in [route_v2_data(&args), shared_route_data(&args)] {
            route_minimum_check(JUPITER_AGGREGATOR_V6, &data, 989_010)
                .expect("a minimum at the floor passes");
            assert!(matches!(
                route_minimum_check(JUPITER_AGGREGATOR_V6, &data, 989_011),
                Err(SwapRefusal::RouteMinimumBelowRequest {
                    index: 0,
                    route_minimum: 989_010,
                    min_output: 989_011
                })
            ));
        }
    }

    #[test]
    fn the_route_minimum_is_the_quote_less_slippage_then_the_platform_fee() {
        // No platform fee: 990,000.
        let data = route_v2_data(&route_args(5_000_000, 1_000_000, 100, 0));
        route_minimum_check(JUPITER_AGGREGATOR_V6, &data, 990_000).expect("floor");
        assert!(route_minimum_check(JUPITER_AGGREGATOR_V6, &data, 990_001).is_err());
        // Full slippage leaves nothing.
        let data = shared_route_data(&route_args(5_000_000, 1_000_000, 10_000, 0));
        assert!(matches!(
            route_minimum_check(JUPITER_AGGREGATOR_V6, &data, 1),
            Err(SwapRefusal::RouteMinimumBelowRequest {
                route_minimum: 0,
                ..
            })
        ));
    }

    #[test]
    fn an_unrecognised_jupiter_form_or_another_program_is_not_read() {
        let args = route_args(5_000, 1_000, 10_000, 0);
        // Terms that would be refused, behind a discriminator that is not a
        // v2 route.
        let unknown = [[0u8; 8].as_slice(), &args].concat();
        route_minimum_check(JUPITER_AGGREGATOR_V6, &unknown, 990_000).expect("unknown form");
        // The v2 form's discriminator on a program that is not Jupiter.
        route_minimum_check(MEMO, &route_v2_data(&args), 990_000).expect("another program");
        route_minimum_check(MEMO, &shared_route_data(&args), 990_000).expect("another program");
        // A recognised discriminator without the arguments, or no data.
        route_minimum_check(JUPITER_AGGREGATOR_V6, &route_v2_data(&args[..19]), 990_000)
            .expect("too short");
        route_minimum_check(
            JUPITER_AGGREGATOR_V6,
            &shared_route_data(&args[..19]),
            990_000,
        )
        .expect("too short");
        route_minimum_check(JUPITER_AGGREGATOR_V6, &[], 990_000).expect("no data");
    }

    /// Set the slippage of the one Jupiter route in the recorded
    /// transaction.
    fn set_recorded_slippage(case: &mut Case, slippage_bps: u16) {
        let mut message = case.message();
        let ix = message
            .instructions()
            .iter()
            .position(|ix| {
                ix.data.starts_with(&ROUTE_V2) || ix.data.starts_with(&SHARED_ACCOUNTS_ROUTE_V2)
            })
            .expect("fixture route instruction");
        let shared = message.instructions()[ix]
            .data
            .starts_with(&SHARED_ACCOUNTS_ROUTE_V2);
        // Discriminator, `id` for the shared form, `in_amount`, then
        // `quoted_out_amount`.
        let offset = 8 + usize::from(shared) + 16;
        match &mut message {
            VersionedMessage::V0(message) => message.instructions[ix].data[offset..offset + 2]
                .copy_from_slice(&slippage_bps.to_le_bytes()),
            VersionedMessage::Legacy(message) => message.instructions[ix].data[offset..offset + 2]
                .copy_from_slice(&slippage_bps.to_le_bytes()),
        }
        case.set_message(message);
    }

    #[tokio::test]
    async fn a_recorded_route_with_slippage_set_wide_is_refused() {
        for fixture in [HNT_USDC, SOL_USDC] {
            let mut case = case(fixture);
            set_recorded_slippage(&mut case, 10_000);
            assert!(
                matches!(
                    case.refusal().await,
                    SwapRefusal::RouteMinimumBelowRequest { .. }
                ),
                "{fixture:.20}"
            );
        }
    }

    // ---- lazy distributor ----

    const LAZY_DISTRIBUTOR: Pubkey = pubkey!("1azyuavdMyvsivtNxPoz6SucD18eDHeXzFCUPq5XU7w");
    const BUBBLEGUM: Pubkey = pubkey!("BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY");

    #[test]
    fn a_lazy_distributor_instruction_naming_the_wallet_is_refused_at_the_top_level() {
        let taker = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        assert_eq!(programs::lazy_distributor::ID, LAZY_DISTRIBUTOR);
        hotspot_check(
            &top_level(&taker, LAZY_DISTRIBUTOR, &[other]),
            json!([]),
            &taker,
        )
        .expect("a lazy distributor instruction not naming the wallet passes");
        assert!(matches!(
            hotspot_check(
                &top_level(&taker, LAZY_DISTRIBUTOR, &[other, taker]),
                json!([]),
                &taker
            ),
            Err(SwapRefusal::HotspotProgramInstruction { program }) if program == LAZY_DISTRIBUTOR
        ));
        assert!(matches!(
            hotspot_check(
                &top_level(&taker, BUBBLEGUM, &[taker]),
                json!([]),
                &taker
            ),
            Err(SwapRefusal::HotspotProgramInstruction { program }) if program == BUBBLEGUM
        ));
    }

    #[test]
    fn an_inner_lazy_distributor_instruction_naming_the_wallet_is_refused() {
        let taker = Pubkey::new_unique();
        let message = top_level(&taker, MEMO, &[]);
        let group = |ix: Value| json!([{ "index": 0, "instructions": [ix] }]);
        for height in [2, 4] {
            assert!(
                matches!(
                    hotspot_check(
                        &message,
                        group(program_call(&LAZY_DISTRIBUTOR, &[taker], height)),
                        &taker
                    ),
                    Err(SwapRefusal::HotspotProgramInstruction { program })
                        if program == LAZY_DISTRIBUTOR
                ),
                "height {height}"
            );
        }
        let parsed = json!({
            "parsed": { "info": { "owner": taker.to_string() }, "type": "update" },
            "program": "lazy-distributor",
            "programId": LAZY_DISTRIBUTOR.to_string(),
            "stackHeight": 3,
        });
        assert!(matches!(
            hotspot_check(&message, group(parsed), &taker),
            Err(SwapRefusal::HotspotProgramInstruction { program }) if program == LAZY_DISTRIBUTOR
        ));
        hotspot_check(
            &message,
            group(program_call(&LAZY_DISTRIBUTOR, &[Pubkey::new_unique()], 3)),
            &taker,
        )
        .expect("a lazy distributor instruction not naming the wallet passes");
    }

    #[test]
    fn a_parsed_instruction_naming_the_wallet_in_an_array_is_refused() {
        let taker = Pubkey::new_unique();
        let message = top_level(&taker, MEMO, &[]);
        let parsed = |names: Vec<String>| {
            json!([{ "index": 0, "instructions": [{
                "parsed": { "info": { "signers": names }, "type": "transfer" },
                "program": "bubblegum",
                "programId": BUBBLEGUM.to_string(),
                "stackHeight": 3,
            }] }])
        };
        assert!(matches!(
            hotspot_check(
                &message,
                parsed(vec![Pubkey::new_unique().to_string(), taker.to_string()]),
                &taker
            ),
            Err(SwapRefusal::HotspotProgramInstruction { .. })
        ));
        hotspot_check(
            &message,
            parsed(vec![Pubkey::new_unique().to_string()]),
            &taker,
        )
        .expect("an array not naming the wallet passes");
    }

    #[test]
    fn a_compiled_inner_instruction_reads_accounts_from_lookup_tables_in_order() {
        use crate::solana_sdk::message::{
            v0::{self, MessageAddressTableLookup},
            MessageHeader,
        };
        let taker = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        let table = Pubkey::new_unique();
        // The table holds `other` then Bubblegum; `other` is loaded writable
        // and Bubblegum read-only, so the keys run: static (the taker and
        // Memo), then the writable entry, then the read-only one.
        let entries = BTreeMap::from([
            ("0".to_string(), other.to_string()),
            ("1".to_string(), BUBBLEGUM.to_string()),
        ]);
        let message = VersionedMessage::V0(v0::Message {
            header: MessageHeader {
                num_required_signatures: 1,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 1,
            },
            account_keys: vec![taker, MEMO],
            recent_blockhash: Hash::default(),
            instructions: vec![],
            address_table_lookups: vec![MessageAddressTableLookup {
                account_key: table,
                writable_indexes: vec![0],
                readonly_indexes: vec![1],
            }],
        });
        let resolved =
            resolve_accounts(&message, &[Some(lookup_table(&entries))]).expect("resolve");
        assert_eq!(resolved.keys, [taker, MEMO, other, BUBBLEGUM]);
        let compiled = |program: u8, account: u8| {
            serde_json::from_value::<Vec<UiInnerInstructions>>(json!([{
                "index": 0,
                "instructions": [{
                    "programIdIndex": program,
                    "accounts": [account],
                    "data": "3Rg2bKuTGTyGGVTrjBjM",
                    "stackHeight": 2,
                }],
            }]))
            .expect("inner instructions")
        };
        assert!(matches!(
            check_hotspot_programs(&message, &resolved.keys, &compiled(3, 0), &taker),
            Err(SwapRefusal::HotspotProgramInstruction { program }) if program == BUBBLEGUM
        ));
        check_hotspot_programs(&message, &resolved.keys, &compiled(2, 0), &taker)
            .expect("the writable entry is not Bubblegum");
    }

    // ---- token account delegation ----

    #[tokio::test]
    async fn token_account_delegated_amount_or_state_change_is_refused() {
        let mut case_amount = case(HNT_USDC);
        let hnt = case_amount.token_account_of(&HNT);
        case_amount.edit_post_token(&hnt, |token| token.delegated_amount = 5);
        assert!(matches!(
            case_amount.refusal().await,
            SwapRefusal::TokenDelegationChanged { account } if account == hnt
        ));

        let mut case_state = case(HNT_USDC);
        case_state.edit_post_token(&hnt, |token| token.state = AccountState::Frozen);
        assert!(matches!(
            case_state.refusal().await,
            SwapRefusal::TokenDelegationChanged { account } if account == hnt
        ));
    }
}
