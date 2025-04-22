pub mod asset;
pub mod b64;
pub mod client;

pub mod boosting;
pub mod dao;
pub mod dc;
pub mod ed25519_instruction;
pub mod entity_key;
pub mod error;
pub mod hotspot;
pub mod keypair;
pub mod kta;
pub mod memo;
pub mod message;
pub mod onboarding;
pub mod priority_fee;
pub mod programs;
pub mod reward;
pub mod token;
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

use client::SolanaRpcClient;
use error::Error;
use keypair::Pubkey;
use solana_sdk::{instruction::Instruction, transaction::Transaction};
use std::{ops::RangeInclusive, sync::Arc};

pub fn init(solana_client: Arc<client::SolanaRpcClient>) -> Result<(), error::Error> {
    kta::init(solana_client)
}

pub struct TransactionOpts {
    pub min_priority_fee: u64,
    pub max_priority_fee: u64,
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
