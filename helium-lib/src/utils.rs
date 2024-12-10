use solana_sdk::instruction::Instruction;

const EPOCH_LENGTH: u64 = 60 * 60 * 24;
pub fn get_current_epoch(unix_time: u64) -> u64 {
    unix_time / EPOCH_LENGTH
}

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
