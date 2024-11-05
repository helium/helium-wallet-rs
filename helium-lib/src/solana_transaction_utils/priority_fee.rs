use anchor_lang::prelude::AccountMeta;
use itertools::Itertools;
use solana_sdk::{
    instruction::Instruction, message::Message, pubkey::Pubkey, signers::Signers,
    transaction::Transaction,
};
use std::ops::Deref;

use crate::{
    anchor_client::RequestBuilder, anchor_lang::ToAccountMetas, client::SolanaRpcClient,
    error::Error, solana_sdk::signer::Signer, utils::replace_or_insert_instruction,
};

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
        .filter(|account_meta| account_meta.is_writable)
        .map(|x| x.pubkey)
        .unique()
        .take(MAX_RECENT_PRIORITY_FEE_ACCOUNTS)
        .collect();

    let recent_fees = client
        .as_ref()
        .get_recent_prioritization_fees(&account_keys)
        .await?;

    let mut max_per_slot: Vec<_> = recent_fees
        .into_iter()
        .group_by(|x| x.slot)
        .into_iter()
        .filter_map(|(slot, fees)| {
            fees.map(|x| x.prioritization_fee)
                .max()
                .map(|max_fee| (slot, max_fee))
        })
        .collect();

    // Only take the most recent 20 maximum fees:
    max_per_slot.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut max_fees: Vec<_> = max_per_slot
        .into_iter()
        .take(20)
        .map(|(_, fee)| fee)
        .collect();
    max_fees.sort();

    // Calculate the median fee
    let estimate = match max_fees.len() {
        0 => min_priority_fee,
        len if len % 2 == 0 => (max_fees[len / 2 - 1] + max_fees[len / 2]) / 2,
        len => max_fees[len / 2],
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
    if let Some(err) = simulation_result.value.err {
        println!("Error: {}", err);
        if let Some(logs) = simulation_result.value.logs {
            for log in logs {
                println!("Log: {}", log);
            }
        }
    }

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

    let compute_price_ix = compute_price_instruction_for_accounts(client, &accounts).await?;

    replace_or_insert_instruction(&mut updated_instructions, compute_budget_ix, 0);
    replace_or_insert_instruction(&mut updated_instructions, compute_price_ix, 1);
    Ok(updated_instructions)
}
