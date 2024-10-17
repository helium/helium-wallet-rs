use crate::{
    anchor_client::RequestBuilder, anchor_lang::ToAccountMetas, client::SolanaRpcClient,
    error::Error, solana_sdk::signer::Signer,
};
use itertools::Itertools;
use std::ops::Deref;

pub const MAX_RECENT_PRIORITY_FEE_ACCOUNTS: usize = 128;
pub const MIN_PRIORITY_FEE: u64 = 1;

pub async fn get_estimate<C: AsRef<SolanaRpcClient>>(
    client: &C,
    accounts: &impl ToAccountMetas,
) -> Result<u64, Error> {
    get_estimate_with_min(client, accounts, MIN_PRIORITY_FEE).await
}

pub async fn get_estimate_with_min<C: AsRef<SolanaRpcClient>>(
    client: &C,
    accounts: &impl ToAccountMetas,
    min_priority_fee: u64,
) -> Result<u64, Error> {
    let account_keys: Vec<_> = accounts
        .to_account_metas(None)
        .into_iter()
        .map(|x| x.pubkey)
        .unique()
        .take(MAX_RECENT_PRIORITY_FEE_ACCOUNTS)
        .collect();
    let recent_fees = client
        .as_ref()
        .get_recent_prioritization_fees(&account_keys)
        .await?;
    let mut max_per_slot = Vec::new();
    for (slot, fees) in &recent_fees.into_iter().group_by(|x| x.slot) {
        let Some(maximum) = fees.map(|x| x.prioritization_fee).max() else {
            continue;
        };
        max_per_slot.push((slot, maximum));
    }
    // Only take the most recent 20 maximum fees:
    max_per_slot.sort_by(|a, b| a.0.cmp(&b.0).reverse());
    let mut max_per_slot: Vec<_> = max_per_slot.into_iter().take(20).map(|x| x.1).collect();
    max_per_slot.sort();
    // Get the median:
    let num_recent_fees = max_per_slot.len();
    let mid = num_recent_fees / 2;
    let estimate = if num_recent_fees == 0 {
        min_priority_fee
    } else if num_recent_fees % 2 == 0 {
        // If the number of samples is even, taken the mean of the two median fees
        (max_per_slot[mid - 1] + max_per_slot[mid]) / 2
    } else {
        max_per_slot[mid]
    }
    .max(min_priority_fee);
    Ok(estimate)
}

pub trait SetPriorityFees {
    fn compute_budget(self, limit: u32) -> Self;
    fn compute_price(self, priority_fee: u64) -> Self;
}

pub fn compute_budget_instruction(compute_limit: u32) -> solana_sdk::instruction::Instruction {
    solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(compute_limit)
}

pub fn compute_price_instruction(priority_fee: u64) -> solana_sdk::instruction::Instruction {
    solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_price(priority_fee)
}

pub async fn compute_price_instruction_for_accounts<C: AsRef<SolanaRpcClient>>(
    client: &C,
    accounts: &impl ToAccountMetas,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    let priority_fee = get_estimate(client, accounts).await?;
    Ok(compute_price_instruction(priority_fee))
}

impl<C: Deref<Target = impl Signer> + Clone> SetPriorityFees for RequestBuilder<'_, C> {
    fn compute_budget(self, compute_limit: u32) -> Self {
        self.instruction(compute_budget_instruction(compute_limit))
    }

    fn compute_price(self, priority_fee: u64) -> Self {
        self.instruction(compute_price_instruction(priority_fee))
    }
}
