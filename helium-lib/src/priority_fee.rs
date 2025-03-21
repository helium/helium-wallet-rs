use std::ops::RangeInclusive;

use crate::{
    anchor_lang::ToAccountMetas,
    client::{SolanaRpcClient, SOLANA_URL_MAINNET},
    error::Error,
    keypair::Pubkey,
    solana_client,
};
use itertools::Itertools;
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount,
    instruction::{AccountMeta, Instruction},
    message::{v0, VersionedMessage},
    signature::NullSigner,
    transaction::VersionedTransaction,
};

pub const MAX_RECENT_PRIORITY_FEE_ACCOUNTS: usize = 128;
pub const MIN_PRIORITY_FEE: u64 = 1;
pub const MAX_PRIORITY_FEE: u64 = 2500000;

pub async fn get_estimate<C: AsRef<SolanaRpcClient>>(
    client: &C,
    accounts: &impl ToAccountMetas,
    fee_range: RangeInclusive<u64>,
) -> Result<u64, Error> {
    let client_url = client.as_ref().url();
    if client_url == SOLANA_URL_MAINNET || client_url.contains("mainnet.helius") {
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

pub async fn compute_budget_for_instructions<C: AsRef<SolanaRpcClient>>(
    client: &C,
    instructions: &[Instruction],
    compute_multiplier: f32,
    payer: &Pubkey,
    blockhash: Option<solana_program::hash::Hash>,
    lookup_tables: Option<Vec<AddressLookupTableAccount>>,
) -> Result<solana_sdk::instruction::Instruction, crate::error::Error> {
    // Check for existing compute unit limit instruction and replace it if found
    let mut updated_instructions = instructions.to_vec();
    let mut has_compute_budget = false;
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
            has_compute_budget = true;
            break;
        }
    }

    if !has_compute_budget {
        // Prepend compute budget instruction if none was found
        updated_instructions.insert(
            0,
            solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(1900000),
        );
    }

    let blockhash_actual = match blockhash {
        Some(hash) => hash,
        None => client.as_ref().get_latest_blockhash().await?,
    };

    let message = VersionedMessage::V0(v0::Message::try_compile(
        payer,
        &updated_instructions,
        lookup_tables.unwrap_or_default().as_slice(),
        blockhash_actual,
    )?);

    let num_signers = updated_instructions
        .iter()
        .flat_map(|ix| ix.accounts.iter())
        .filter(|a| a.is_signer)
        .map(|a| a.pubkey)
        .chain(std::iter::once(*payer)) // Include payer
        .unique()
        .count();

    let signers = (0..num_signers)
        .map(|_| NullSigner::new(payer))
        .collect::<Vec<_>>();

    let null_signers: Vec<&NullSigner> = signers.iter().collect();
    let snub_tx = VersionedTransaction::try_new(message, null_signers.as_slice())?;

    // Simulate the transaction to get the actual compute used
    let simulation_result = client.as_ref().simulate_transaction(&snub_tx).await?;
    if let Some(err) = simulation_result.value.err {
        return Err(Error::SimulatedTransactionError(err));
    }
    let actual_compute_used = simulation_result.value.units_consumed.unwrap_or(200000);
    let final_compute_budget = (actual_compute_used as f32 * compute_multiplier) as u32;
    Ok(compute_budget_instruction(final_compute_budget))
}

pub async fn auto_compute_limit_and_price<C: AsRef<SolanaRpcClient>>(
    client: &C,
    instructions: &[Instruction],
    compute_multiplier: f32,
    payer: &Pubkey,
    blockhash: Option<solana_program::hash::Hash>,
    lookup_tables: Option<Vec<AddressLookupTableAccount>>,
) -> Result<Vec<Instruction>, Error> {
    let mut updated_instructions = instructions.to_vec();

    // Compute budget instruction
    let compute_budget_ix = compute_budget_for_instructions(
        client,
        &updated_instructions,
        compute_multiplier,
        payer,
        blockhash,
        lookup_tables,
    )
    .await?;

    // Compute price instruction
    let accounts: Vec<AccountMeta> = instructions
        .iter()
        .flat_map(|i| i.accounts.iter().map(|a| a.pubkey))
        .unique()
        .map(|pk| AccountMeta::new(pk, false))
        .collect();

    let compute_price_ix = compute_price_instruction_for_accounts(
        client,
        &accounts,
        RangeInclusive::new(MIN_PRIORITY_FEE, MAX_PRIORITY_FEE),
    )
    .await?;

    insert_or_replace_compute_instructions(
        &mut updated_instructions,
        compute_budget_ix,
        compute_price_ix,
    );

    Ok(updated_instructions)
}

fn insert_or_replace_compute_instructions(
    instructions: &mut Vec<Instruction>,
    budget_ix: Instruction,
    price_ix: Instruction,
) {
    if let Some(pos) = instructions
        .iter()
        .position(|ix| ix.program_id == solana_sdk::compute_budget::id())
    {
        instructions[pos] = budget_ix;
        instructions[pos + 1] = price_ix;
    } else {
        instructions.insert(0, budget_ix);
        instructions.insert(1, price_ix);
    }
}
