use crate::{
    client::SolanaClient,
    error::Error,
    priority_fee::auto_compute_limit_and_price,
    solana_client::{
        nonblocking::tpu_client::TpuClient,
        send_and_confirm_transactions_in_parallel::{
            send_and_confirm_transactions_in_parallel, SendAndConfirmConfig,
        },
        tpu_client::TpuClientConfig,
    },
    solana_sdk::{
        commitment_config::CommitmentConfig, compute_budget::ComputeBudgetInstruction,
        instruction::Instruction, message::Message, signature::Keypair, signer::Signer,
        transaction::Transaction,
    },
};

const MAX_TRANSACTION_SIZE: usize = 1232; // Maximum transaction size in bytes

pub fn replace_or_insert_instruction(
    instructions: &mut Vec<Instruction>,
    new_instruction: Instruction,
    insert_pos: usize,
) {
    if let Some(pos) = instructions
        .iter()
        .position(|ix| ix.program_id == solana_sdk::compute_budget::id())
    {
        instructions[pos + insert_pos] = new_instruction;
    } else {
        instructions.insert(insert_pos, new_instruction);
    }
}

// Returns packed txs with the indices in instructions that were used in that tx.
pub fn pack_instructions(
    instructions: Vec<Vec<Instruction>>,
    payer: &Keypair,
) -> Vec<(Vec<Instruction>, Vec<usize>)> {
    // Change return type
    let mut transactions = Vec::new();
    let compute_ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(200000),
        ComputeBudgetInstruction::set_compute_unit_price(1),
    ];
    let mut curr_instructions: Vec<Instruction> = compute_ixs.clone();
    let mut curr_indices: Vec<usize> = Vec::new(); // Track indices of instructions
    let mut ix_queue: Vec<(Instruction, usize)> = instructions
        .iter()
        .enumerate()
        .flat_map(|(i, group)| group.iter().map(move |ix| (ix.clone(), i)))
        .collect();
    ix_queue.reverse();

    while let Some((ix, index)) = ix_queue.pop() {
        curr_instructions.push(ix);
        curr_indices.push(index);
        let tx = Transaction::new_with_payer(&curr_instructions, Some(&payer.pubkey()));
        let len = bincode::serialize(&tx).unwrap().len();
        if len > MAX_TRANSACTION_SIZE {
            ix_queue.push((
                curr_instructions.pop().unwrap(),
                curr_indices.pop().unwrap(),
            ));
            transactions.push((curr_instructions.clone(), curr_indices.clone()));
            curr_instructions = compute_ixs.clone();
            curr_indices.clear();
        }
    }

    if !curr_instructions.is_empty() {
        transactions.push((curr_instructions.clone(), curr_indices.clone()));
    }

    transactions
}

pub async fn send_instructions(
    client: SolanaClient,
    ixs: Vec<Instruction>,
    extra_signers: &[Keypair],
    sequentially: bool,
) -> Result<(), Error> {
    let wallet = client
        .wallet
        .as_ref()
        .ok_or_else(|| Error::WalletUnconfigured)?;

    let (blockhash, _) = client
        .inner
        .as_ref()
        .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
        .await
        .expect("Failed to get latest blockhash");

    let txs = pack_instructions(vec![ixs], &wallet);
    let mut with_auto_compute: Vec<Message> = Vec::new();
    let keys: Vec<&dyn Signer> = std::iter::once(&wallet as &dyn Signer)
        .chain(extra_signers.iter().map(|k| k as &dyn Signer))
        .collect();

    for (tx, _) in &txs {
        let computed = auto_compute_limit_and_price(
            &client,
            tx.clone(),
            &keys,
            1.2,
            Some(&wallet.pubkey()),
            Some(blockhash),
        )
        .await
        .unwrap();

        with_auto_compute.push(Message::new(&computed, Some(&wallet.pubkey())));
    }

    if with_auto_compute.is_empty() {
        return Ok(());
    }

    let results;
    let tpu_client = TpuClient::new(
        "helium-lib",
        client.inner.clone(),
        &client.ws_url(),
        TpuClientConfig::default(),
    )
    .await?;

    match sequentially {
        true => {
            results = tpu_client
                .send_and_confirm_messages_with_spinner(&with_auto_compute, &keys)
                .await?;
        }
        false => {
            results = send_and_confirm_transactions_in_parallel(
                client.inner.clone(),
                Some(tpu_client),
                &with_auto_compute,
                &keys,
                SendAndConfirmConfig {
                    with_spinner: true,
                    resign_txs_count: Some(5),
                },
            )
            .await?;
        }
    }

    if let Some(err) = results.into_iter().flatten().next() {
        return Err(Error::from(err));
    }

    Ok(())
}
