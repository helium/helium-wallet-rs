use crate::{
    message,
    solana_sdk::{
        address_lookup_table::AddressLookupTableAccount, instruction::Instruction, signers::Signers,
    },
    Error,
};

pub use solana_sdk::transaction::VersionedTransaction;
pub use solana_transaction_utils::pack::PackedTransaction;

pub fn mk_transaction<T: Signers + ?Sized>(
    msg: message::VersionedMessage,
    signers: &T,
) -> Result<VersionedTransaction, Error> {
    VersionedTransaction::try_new(msg, signers).map_err(Error::from)
}

pub fn pack_instructions(
    instructions: &[&[Instruction]],
    lookup_tables: Option<Vec<AddressLookupTableAccount>>,
) -> Result<Vec<PackedTransaction>, Error> {
    solana_transaction_utils::pack::pack_instructions_into_transactions(instructions, lookup_tables)
        .map_err(Error::from)
}
