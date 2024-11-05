use solana_sdk::{
    compute_budget::ComputeBudgetInstruction, instruction::Instruction, signature::Keypair,
    signer::Signer, transaction::Transaction,
};

const MAX_TRANSACTION_SIZE: usize = 1232; // Maximum transaction size in bytes

// Returns packed txs with the indices in instructions that were used in that tx.
pub fn pack_instructions_into_transactions(
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
