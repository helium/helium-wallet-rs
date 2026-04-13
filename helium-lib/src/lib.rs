//! Client library for interacting with the Helium network on Solana.
//!
//! Provides token operations, hotspot management, onboarding, rewards,
//! and transaction building for the Helium ecosystem.

#![forbid(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

/// Compressed NFT asset operations (DAS API).
pub mod asset;
/// Base64 encoding/decoding utilities.
pub mod b64;
/// Solana RPC and DAS client wrappers.
pub mod client;

/// Hex boosting activation for mobile coverage areas.
pub mod boosting;
/// Helium DAO and sub-DAO account lookups.
pub mod dao;
/// Data Credit minting, delegation, and burning.
pub mod dc;
/// Ed25519 signature verification instructions.
pub mod ed25519_instruction;
/// Entity key encoding for hotspots and other network entities.
pub mod entity_key;
/// Error types used throughout the library.
pub mod error;
/// Hotspot onboarding, configuration, and info queries.
pub mod hotspot;
/// Jupiter DEX swap integration.
pub mod jupiter;
/// Solana keypair management with optional BIP39 mnemonic support.
pub mod keypair;
/// Key-to-asset (KTA) account lookups and caching.
pub mod kta;
/// Transaction memo encoding.
pub mod memo;
/// Versioned message construction with address lookup tables.
pub mod message;
/// Maker onboarding server client.
pub mod onboarding;
/// Compute unit price estimation for transaction priority fees.
pub mod priority_fee;
/// Anchor program ID and account definitions.
pub mod programs;
/// Reward claim queuing via task queues.
pub mod queue;
/// Reward claim and oracle interactions.
pub mod reward;
/// Cron-based scheduled reward claiming.
pub mod schedule;
/// Token operations: transfers, burns, balances, and prices.
pub mod token;
/// Transaction building, signing, and confirmation.
pub mod transaction;

pub use crate::programs::{
    bubblegum, circuit_breaker, data_credits, helium_entity_manager, helium_sub_daos, hexboosting,
    lazy_distributor, rewards_oracle, spl_account_compression,
};
pub use anchor_client;
pub use anchor_client::solana_client;
pub use anchor_lang;
pub use anchor_spl;
pub use solana_program;
pub use solana_sdk;
pub use solana_sdk::bs58;
pub use solana_transaction_status;
pub use tuktuk_sdk;

pub(crate) trait Zero {
    const ZERO: Self;
}

impl Zero for u32 {
    const ZERO: Self = 0;
}

impl Zero for i32 {
    const ZERO: Self = 0;
}

impl Zero for u16 {
    const ZERO: Self = 0;
}

impl Zero for u64 {
    const ZERO: Self = 0;
}

impl Zero for rust_decimal::Decimal {
    const ZERO: Self = rust_decimal::Decimal::ZERO;
}

pub(crate) fn is_zero<T>(value: &T) -> bool
where
    T: PartialEq + Zero,
{
    value == &T::ZERO
}

use client::SolanaRpcClient;
use error::Error;
use keypair::Pubkey;
use solana_sdk::{instruction::Instruction, transaction::Transaction};
use std::{ops::RangeInclusive, sync::Arc};

/// Initializes the global KTA (key-to-asset) cache.
///
/// Must be called before any KTA lookups. Requires an active Solana RPC client.
pub fn init(solana_client: Arc<client::SolanaRpcClient>) -> Result<(), error::Error> {
    kta::init(solana_client)
}

/// Options controlling transaction priority fees and address lookup tables.
pub struct TransactionOpts {
    /// Minimum priority fee in micro-lamports per compute unit.
    pub min_priority_fee: u64,
    /// Maximum priority fee in micro-lamports per compute unit.
    pub max_priority_fee: u64,
    /// Address lookup tables to include for transaction compression.
    pub lut_addresses: Vec<Pubkey>,
}

impl Default for TransactionOpts {
    fn default() -> Self {
        Self {
            min_priority_fee: priority_fee::MIN_PRIORITY_FEE,
            max_priority_fee: priority_fee::MAX_PRIORITY_FEE,
            lut_addresses: vec![message::COMMON_LUT],
        }
    }
}

impl TransactionOpts {
    fn fee_range(&self) -> RangeInclusive<u64> {
        RangeInclusive::new(self.min_priority_fee, self.max_priority_fee)
    }
}

/// Creates a transaction with a fresh blockhash, returning the transaction and block height.
pub async fn mk_transaction_with_blockhash<C: AsRef<SolanaRpcClient>>(
    client: &C,
    ixs: &[Instruction],
    payer: &Pubkey,
) -> Result<(Transaction, u64), Error> {
    let mut txn = Transaction::new_with_payer(ixs, Some(payer));
    let solana_client = AsRef::<SolanaRpcClient>::as_ref(client);
    let (latest_blockhash, latest_block_height) = solana_client
        .get_latest_blockhash_with_commitment(solana_client.commitment())
        .await?;
    txn.message.recent_blockhash = latest_blockhash;
    Ok((txn, latest_block_height))
}
