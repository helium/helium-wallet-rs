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

pub fn entity_creator_key(dao: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"entity_creator", dao.as_ref()],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn rewardable_entity_config_key(sub_dao: &Pubkey, symbol: &str) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"rewardable_entity_config",
            sub_dao.as_ref(),
            symbol.as_bytes(),
        ],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn hotspot_collection_key(maker_or_data_only: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"collection", maker_or_data_only.as_ref()],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn data_only_config_key(dao: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"data_only_config", dao.as_ref()],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn data_only_escrow_key(data_only: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"data_only_escrow", data_only.as_ref()],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn maker_key(dao: &Pubkey, name: &str) -> Pubkey {
    Pubkey::find_program_address(
        &[b"maker", dao.as_ref(), name.as_bytes()],
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

pub fn maker_approval_key(rewardable_entity_config: &Pubkey, maker: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"maker_approval",
            rewardable_entity_config.as_ref(),
            maker.as_ref(),
        ],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn key_to_asset_key_raw(dao: &Pubkey, hashed_entity_key: &[u8]) -> Pubkey {
    Pubkey::find_program_address(
        &[b"key_to_asset", dao.as_ref(), hashed_entity_key],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn iot_config_key(sub_dao: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"iot_config", sub_dao.as_ref()],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn iot_info_key(rewardable_entity_config: &Pubkey, entity_key: &[u8]) -> Pubkey {
    let hash = Sha256::digest(entity_key);
    Pubkey::find_program_address(
        &[b"iot_info", rewardable_entity_config.as_ref(), &hash],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn mobile_config_key(sub_dao: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"mobile_config", sub_dao.as_ref()],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}

pub fn mobile_info_key(rewardable_entity_config: &Pubkey, entity_key: &[u8]) -> Pubkey {
    let hash = Sha256::digest(entity_key);
    Pubkey::find_program_address(
        &[b"mobile_info", rewardable_entity_config.as_ref(), &hash],
        &helium_anchor_gen::helium_entity_manager::ID,
    )
    .0
}
