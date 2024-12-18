use crate::keypair::Pubkey;
use sha2::{Digest, Sha256};

pub use helium_anchor_gen::helium_entity_manager::*;

pub fn shared_merkle_key(proof_size: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[b"shared_merkle", &[proof_size]],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn program_approval_key(dao: &Pubkey, program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"program_approval", dao.as_ref(), program.as_ref()],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}
