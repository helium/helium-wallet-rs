use crate::{
    anchor_lang::ToAccountMetas,
    client::SolanaRpcClient,
    error::Error,
    keypair::Pubkey,
    solana_client,
    solana_sdk::{
        instruction::{AccountMeta, Instruction},
        message::Message,
        signers::Signers,
        transaction::Transaction,
    },
    transaction::replace_or_insert_instruction,
};
use itertools::Itertools;

pub const MAX_RECENT_PRIORITY_FEE_ACCOUNTS: usize = 128;
pub const MIN_PRIORITY_FEE: u64 = 1;

pub async fn get_estimate_with_min<C: AsRef<SolanaRpcClient>>(
    client: &C,
    accounts: &impl ToAccountMetas,
    min_priority_fee: u64,
) -> Result<u64, Error> {
    let client_url = client.as_ref().url();
    if client_url.contains("mainnet.helius") {
        helius::get_estimate_with_min(client, accounts, min_priority_fee).await
    } else {
        base::get_estimate_with_min(client, accounts, min_priority_fee).await
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

    pub async fn get_estimate_with_min<C: AsRef<SolanaRpcClient>>(
        client: &C,
        accounts: &impl ToAccountMetas,
        min_priority_fee: u64,
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
                }
            }
        ]);

        let response: Response = client.as_ref().send(request, params).await?;
        Ok((response.priority_fee_estimate.ceil() as u64).max(min_priority_fee))
    }
}

mod base {
    use super::*;

    pub async fn get_estimate_with_min<C: AsRef<SolanaRpcClient>>(
        client: &C,
        accounts: &impl ToAccountMetas,
        min_priority_fee: u64,
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
    min_priority_fee: u64,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    let priority_fee = get_estimate_with_min(client, accounts, min_priority_fee).await?;
    Ok(compute_price_instruction(priority_fee))
}

pub async fn compute_budget_for_instructions<C: AsRef<SolanaRpcClient>, T: Signers + ?Sized>(
    client: &C,
    instructions: Vec<Instruction>,
    signers: &T,
    compute_multiplier: f32,
    payer: Option<&Pubkey>,
    blockhash: Option<solana_program::hash::Hash>,
) -> Result<solana_sdk::instruction::Instruction, crate::error::Error> {
    // Check for existing compute unit limit instruction and replace it if found
    let mut updated_instructions = instructions.clone();
    for ix in &mut updated_instructions {
        if ix.program_id == solana_sdk::compute_budget::id()
            && ix.data.first()
                == solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(0)
                    .data
                    .first()
        {
            ix.data = solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(
                1900000,
            )
            .data; // Replace limit
        }
    }

    let blockhash_actual = match blockhash {
        Some(hash) => hash,
        None => client.as_ref().get_latest_blockhash().await?,
    };

    let snub_tx = Transaction::new(
        signers,
        Message::new(&updated_instructions, payer),
        blockhash_actual,
    );

    // Simulate the transaction to get the actual compute used
    let simulation_result = client.as_ref().simulate_transaction(&snub_tx).await?;
    let actual_compute_used = simulation_result.value.units_consumed.unwrap_or(200000);
    let final_compute_budget = (actual_compute_used as f32 * compute_multiplier) as u32;
    Ok(compute_budget_instruction(final_compute_budget))
}

pub async fn auto_compute_limit_and_price<C: AsRef<SolanaRpcClient>, T: Signers + ?Sized>(
    client: &C,
    instructions: Vec<Instruction>,
    signers: &T,
    compute_multiplier: f32,
    payer: Option<&Pubkey>,
    blockhash: Option<solana_program::hash::Hash>,
) -> Result<Vec<Instruction>, Error> {
    let mut updated_instructions = instructions.clone();

    // Compute budget instruction
    let compute_budget_ix = compute_budget_for_instructions(
        client,
        instructions.clone(),
        signers,
        compute_multiplier,
        payer,
        blockhash,
    )
    .await?;

    // Compute price instruction
    let accounts: Vec<AccountMeta> = instructions
        .iter()
        .flat_map(|i| i.accounts.iter().map(|a| a.pubkey))
        .unique()
        .map(|pk| AccountMeta::new(pk, false))
        .collect();

    let compute_price_ix =
        compute_price_instruction_for_accounts(client, &accounts, MIN_PRIORITY_FEE).await?;

    replace_or_insert_instruction(&mut updated_instructions, compute_budget_ix, 0);
    replace_or_insert_instruction(&mut updated_instructions, compute_price_ix, 1);
    Ok(updated_instructions)
}
