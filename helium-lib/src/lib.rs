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
/// HTTP client for the Helium blockchain-api transaction-building service.
pub mod blockchain_api;
/// Solana RPC and DAS client wrappers.
pub mod client;

/// Helium DAO and sub-DAO account lookups.
pub mod dao;
/// Entity key encoding for hotspots and other network entities.
pub mod entity_key;
/// Error types used throughout the library.
pub mod error;
/// Hotspot onboarding, configuration, and info queries.
pub mod hotspot;
/// Solana keypair management with optional BIP39 mnemonic support.
pub mod keypair;
/// Key-to-asset (KTA) account lookups and caching.
pub mod kta;
/// Anchor program ID and account definitions.
pub mod programs;
/// Reward claim queuing via task queues.
pub mod queue;
/// Reward claim and oracle interactions.
pub mod reward;
/// Cron-based scheduled reward claiming.
pub mod schedule;
/// Squads multisig integration (v3 + v4): proposal building, voting, decoding.
pub mod squads;
/// Token operations: transfers, burns, balances, and prices.
pub mod token;
/// Transaction signing and confirmation.
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

use error::Error;
use keypair::Pubkey;
use std::sync::Arc;

/// Initializes the global KTA (key-to-asset) cache.
///
/// Must be called before any KTA lookups. Requires an active Solana RPC client.
pub fn init(solana_client: Arc<client::SolanaRpcClient>) -> Result<(), error::Error> {
    kta::init(solana_client)
}
