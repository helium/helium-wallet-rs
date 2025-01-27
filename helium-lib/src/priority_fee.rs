use std::ops::RangeInclusive;

use crate::{
    anchor_lang::ToAccountMetas, client::SolanaRpcClient, error::Error, keypair::Pubkey,
    solana_client,
};
use itertools::Itertools;

pub const MAX_RECENT_PRIORITY_FEE_ACCOUNTS: usize = 128;
pub const MIN_PRIORITY_FEE: u64 = 1;
pub const MAX_PRIORITY_FEE: u64 = 2500000;

pub async fn get_estimate<C: AsRef<SolanaRpcClient>>(
    client: &C,
    accounts: &impl ToAccountMetas,
    fee_range: RangeInclusive<u64>,
) -> Result<u64, Error> {
    let client_url = client.as_ref().url();
    if client_url.contains("mainnet.helius") {
        helius::get_estimate(client, accounts, fee_range).await
    } else {
        base::get_estimate(client, accounts, fee_range).await
    }
}

fn account_keys(accounts: &impl ToAccountMetas) -> impl Iterator<Item = Pubkey> {
    accounts
        .to_account_metas(None)
        .into_iter()
        .filter(|account_meta| account_meta.is_writable)
        .map(|x| x.pubkey)
        .unique()
        .take(MAX_RECENT_PRIORITY_FEE_ACCOUNTS)
}

mod helius {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    pub async fn get_estimate<C: AsRef<SolanaRpcClient>>(
        client: &C,
        accounts: &impl ToAccountMetas,
        fee_range: RangeInclusive<u64>,
    ) -> Result<u64, Error> {
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Response {
            priority_fee_estimate: f64,
        }
        let request = solana_client::rpc_request::RpcRequest::Custom {
            method: "getPriorityFeeEstimate",
        };
        let account_keys: Vec<_> = account_keys(accounts).map(|v| v.to_string()).collect();
        let params = json!([
            {
                "accountKeys": account_keys,
                "options": {
                    "recommended": true,
                    "evaluateEmptySlotAsZero": true
                }
            }
        ]);

        let response: Response = client.as_ref().send(request, params).await?;
        Ok((response.priority_fee_estimate.ceil() as u64)
            .min(*fee_range.end())
            .max(*fee_range.start()))
    }
}

mod base {
    use super::*;

    pub async fn get_estimate<C: AsRef<SolanaRpcClient>>(
        client: &C,
        accounts: &impl ToAccountMetas,
        fee_range: RangeInclusive<u64>,
    ) -> Result<u64, Error> {
        let account_keys: Vec<_> = account_keys(accounts).collect();
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
            *fee_range.start()
        } else if num_recent_fees % 2 == 0 {
            // If the number of samples is even, taken the mean of the two median fees
            (max_per_slot[mid - 1] + max_per_slot[mid]) / 2
        } else {
            max_per_slot[mid]
        }
        .min(*fee_range.end())
        .max(*fee_range.start());
        Ok(estimate)
    }
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
    fee_range: RangeInclusive<u64>,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    let priority_fee = get_estimate(client, accounts, fee_range).await?;
    Ok(compute_price_instruction(priority_fee))
}
