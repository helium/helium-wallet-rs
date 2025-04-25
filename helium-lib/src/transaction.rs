use crate::{
    error::EncodeError,
    message, priority_fee,
    solana_sdk::{
        address_lookup_table::AddressLookupTableAccount, instruction::Instruction, pubkey::Pubkey,
        signature::NullSigner,
    },
    Error,
};

use solana_sdk::signers::Signers;
pub use solana_sdk::transaction::VersionedTransaction;

pub const MAX_TRANSACTION_SIZE: usize = 1232; // Maximum transaction size in bytes

pub fn mk_transaction<T: Signers + ?Sized>(
    msg: message::VersionedMessage,
    signers: &T,
) -> Result<VersionedTransaction, Error> {
    VersionedTransaction::try_new(msg, signers).map_err(Error::from)
}

#[derive(Debug)]
pub struct PackedTransaction {
    pub instructions: Vec<Instruction>,
    pub task_ids: Vec<usize>,
}

impl Default for PackedTransaction {
    fn default() -> Self {
        Self {
            instructions: priority_fee::compute_placeholder_instructions().to_vec(),
            task_ids: Default::default(),
        }
    }
}

impl PackedTransaction {
    pub fn push(&mut self, instructions: &[Instruction], index: usize) {
        self.instructions.extend_from_slice(instructions);
        self.task_ids.push(index);
    }

    pub fn is_empty(&self) -> bool {
        self.task_ids.is_empty()
    }

    pub fn mk_transaction(
        &self,
        extra_ixs: &[Instruction],
        lookup_tables: &[AddressLookupTableAccount],
        payer: &Pubkey,
    ) -> Result<VersionedTransaction, Error> {
        let ixs = &[&self.instructions, extra_ixs].concat();
        let msg = message::mk_raw_message(ixs, lookup_tables, payer)?;
        mk_transaction(msg, &[&NullSigner::new(payer)])
    }

    pub fn transaction_len(
        &self,
        extra_ixs: &[Instruction],
        lookup_tables: &[AddressLookupTableAccount],
    ) -> Result<usize, Error> {
        let tx = self.mk_transaction(extra_ixs, lookup_tables, &Pubkey::default())?;
        let len = bincode::serialize(&tx)
            .map_err(EncodeError::from)
            .map(|data| data.len())?;
        Ok(len)
    }
}

// Returns packed txs with the indices in instructions that were used in that tx.
pub fn pack_instructions(
    instructions: &[&[Instruction]],
    lookup_tables: Option<Vec<AddressLookupTableAccount>>,
) -> Result<Vec<PackedTransaction>, Error> {
    let mut transactions = Vec::new();
    let mut curr_transaction = PackedTransaction::default();
    let lookup_tables = lookup_tables.unwrap_or_default();

    // Instead of flattening all instructions, process them group by group
    for (group_idx, group) in instructions.iter().enumerate() {
        // Create a test transaction with current instructions + entire new group.
        // If adding the entire group would exceed size limit, start a new transaction
        // (but only if we already have instructions in the current batch)
        if curr_transaction.transaction_len(group, &lookup_tables)? > MAX_TRANSACTION_SIZE
            && !curr_transaction.is_empty()
        {
            transactions.push(curr_transaction);
            curr_transaction = PackedTransaction::default();
        }

        // Add the entire group to current transaction
        curr_transaction.push(group, group_idx);

        let txn_len = curr_transaction.transaction_len(&[], &lookup_tables)?;
        if txn_len > MAX_TRANSACTION_SIZE {
            return Err(EncodeError::too_many_instructions(txn_len).into());
        }
    }

    // Push final transaction if there are remaining instructions
    if !curr_transaction.is_empty() {
        transactions.push(curr_transaction);
    }

    Ok(transactions)
}
