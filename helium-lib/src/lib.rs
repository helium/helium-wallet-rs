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
/// Data-credit minting. Requires the `txn` feature.
pub mod dc;
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
/// Versioned-message assembly with address lookup tables. Requires the `txn`
/// feature.
pub mod message;
/// Priority-fee bounds, plus compute-budget and fee-estimation helpers. The
/// bounds are always available so callers can hold a server-built transaction
/// to the same ceiling the local builder applies; the helpers need `txn`.
pub mod priority_fee;
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
/// Checks a caller applies to a transaction built somewhere else.
pub mod verify;

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

/// Options controlling transaction priority fees and address lookup tables.
///
/// Requires the `txn` feature.
pub struct TransactionOpts {
    /// Minimum priority fee in micro-lamports per compute unit.
    pub min_priority_fee: u64,
    /// Maximum priority fee in micro-lamports per compute unit.
    pub max_priority_fee: u64,
    /// Address lookup tables to include for transaction compression.
    pub lut_addresses: Vec<Pubkey>,
}

/// Returns the default LUT addresses for the cluster identified by `url`,
/// selecting the devnet common LUT for devnet URLs and the mainnet common
/// LUT otherwise. See [`client::is_devnet`] for how the cluster is detected.
fn default_lut_addresses_for_url(url: &str) -> Vec<Pubkey> {
    if client::is_devnet(url) {
        vec![message::COMMON_LUT_DEVNET]
    } else {
        vec![message::COMMON_LUT]
    }
}

impl Default for TransactionOpts {
    /// Default options assuming the **mainnet** cluster. When the target
    /// cluster is not known to be mainnet, build options with
    /// [`TransactionOpts::for_url`] or [`TransactionOpts::for_client`] so the
    /// correct (devnet vs mainnet) common lookup table is selected.
    fn default() -> Self {
        Self {
            min_priority_fee: priority_fee::MIN_PRIORITY_FEE,
            max_priority_fee: priority_fee::MAX_PRIORITY_FEE,
            lut_addresses: vec![message::COMMON_LUT],
        }
    }
}

impl TransactionOpts {
    /// Builds options for the cluster identified by `url`, selecting the
    /// devnet or mainnet common lookup table accordingly. Priority fees use
    /// the same defaults as [`TransactionOpts::default`].
    pub fn for_url(url: &str) -> Self {
        Self {
            lut_addresses: default_lut_addresses_for_url(url),
            ..Self::default()
        }
    }

    /// Builds options for the cluster `client` is connected to, selecting the
    /// devnet or mainnet common lookup table accordingly. Priority fees use
    /// the same defaults as [`TransactionOpts::default`].
    pub fn for_client<C: AsRef<client::SolanaRpcClient>>(client: &C) -> Self {
        Self::for_url(&client.as_ref().url())
    }

    fn fee_range(&self) -> std::ops::RangeInclusive<u64> {
        std::ops::RangeInclusive::new(self.min_priority_fee, self.max_priority_fee)
    }
}

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
