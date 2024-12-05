use crate::{keypair::Pubkey, utils::get_current_epoch};

pub use helium_anchor_gen::helium_sub_daos::*;

pub fn sub_dao_epoch_info_key(sub_dao: &Pubkey, unix_time: u64) -> Pubkey {
    let epoch = get_current_epoch(unix_time);
    let b_u64 = epoch.to_le_bytes();
    Pubkey::find_program_address(
        &[b"sub_dao_epoch_info", sub_dao.as_ref(), &b_u64],
        &helium_anchor_gen::helium_sub_daos::ID,
    )
    .0
}

pub fn dao_epoch_info_key(dao: &Pubkey, unix_time: u64) -> Pubkey {
    let epoch = get_current_epoch(unix_time);
    let b_u64 = epoch.to_le_bytes();
    Pubkey::find_program_address(
        &[b"dao_epoch_info", dao.as_ref(), &b_u64],
        &helium_anchor_gen::helium_sub_daos::ID,
    )
    .0
}

pub fn dao_key(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"dao", mint.as_ref()],
        &helium_anchor_gen::helium_sub_daos::ID,
    )
    .0
}

pub fn sub_dao_key(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"sub_dao", mint.as_ref()],
        &helium_anchor_gen::helium_sub_daos::ID,
    )
    .0
}

pub fn delegated_position_key(position: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"delegated_position", position.as_ref()],
        &helium_anchor_gen::helium_sub_daos::ID,
    )
    .0
}
