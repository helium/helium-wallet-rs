pub mod asset;
pub mod b64;
pub mod client;

pub mod boosting;
pub mod dao;
pub mod dc;
pub mod entity_key;
pub mod error;
pub mod helium_entity_manager;
pub mod helium_sub_daos;
pub mod hotspot;
pub mod iot_routing_manager;
pub mod keypair;
pub mod kta;
pub mod memo;
pub mod metaplex;
pub mod onboarding;
pub mod programs;
pub mod reward;
pub mod solana_transaction_utils;
pub mod token;
pub mod utils;

pub use anchor_client;
pub use anchor_client::solana_client;
pub use anchor_spl;
pub use helium_anchor_gen::{
    anchor_lang, circuit_breaker, data_credits, hexboosting, lazy_distributor, rewards_oracle,
};
pub use solana_sdk;
pub use solana_sdk::bs58;
pub use solana_transaction_utils::priority_fee;

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

impl Zero for rust_decimal::Decimal {
    const ZERO: Self = rust_decimal::Decimal::ZERO;
}

pub(crate) fn is_zero<T>(value: &T) -> bool
where
    T: PartialEq + Zero,
{
    value == &T::ZERO
}

use std::sync::Arc;

pub fn init(solana_client: Arc<client::SolanaClient>) -> Result<(), error::Error> {
    kta::init(solana_client.solana_rpc_client())
}

pub struct TransactionOpts {
    pub min_priority_fee: u64,
}

impl Default for TransactionOpts {
    fn default() -> Self {
        Self {
            min_priority_fee: priority_fee::MIN_PRIORITY_FEE,
        }
    }
}
