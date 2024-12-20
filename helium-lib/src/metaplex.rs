use crate::{keypair::Pubkey, programs::TOKEN_METADATA_PROGRAM_ID};

pub fn bubblegum_signer_key() -> Pubkey {
    Pubkey::find_program_address(&[b"collection_cpi"], &mpl_bubblegum::ID).0
}

pub fn collection_metadata_key(collection_key: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            collection_key.as_ref(),
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    )
    .0
}

pub fn collection_master_edition_key(collection_key: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            collection_key.as_ref(),
            b"edition",
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    )
    .0
}

pub fn merkle_tree_authority_key(merkle_tree: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[merkle_tree.as_ref()], &mpl_bubblegum::ID).0
}
