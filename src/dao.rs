use crate::{hotspot, keypair::Pubkey, result::Result, token::Token};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dao {
    Hnt,
}

impl std::fmt::Display for Dao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&str)
    }
}

impl Dao {
    pub fn key(&self) -> Pubkey {
        let mint = match self {
            Self::Hnt => Token::Hnt.mint(),
        };
        let (dao_key, _) =
            Pubkey::find_program_address(&[b"dao", mint.as_ref()], &helium_sub_daos::id());
        dao_key
    }

    pub fn dataonly_config_key(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"data_only_config", self.key().as_ref()],
            &helium_entity_manager::id(),
        );
        key
    }

    pub fn dataonly_escrow_key(&self) -> Pubkey {
        let (data_only_escrow, _doe_bump) = Pubkey::find_program_address(
            &[b"data_only_escrow", self.dataonly_config_key().as_ref()],
            &helium_entity_manager::id(),
        );
        data_only_escrow
    }

    pub fn collection_metadata_key(&self, collection_key: &Pubkey) -> Pubkey {
        let (collection_metadata, _bump) = Pubkey::find_program_address(
            &[
                b"metadata",
                lazy_distributor::token_metadata::ID.as_ref(),
                collection_key.as_ref(),
            ],
            &lazy_distributor::token_metadata::ID,
        );
        collection_metadata
    }

    pub fn collection_master_edition_key(&self, collection_key: &Pubkey) -> Pubkey {
        let (collection_master_edition, _cme_bump) = Pubkey::find_program_address(
            &[
                b"metadata",
                lazy_distributor::token_metadata::ID.as_ref(),
                collection_key.as_ref(),
                b"edition",
            ],
            &lazy_distributor::token_metadata::ID,
        );
        collection_master_edition
    }

    pub fn merkle_tree_authority(&self, merkle_tree: &Pubkey) -> Pubkey {
        let (tree_authority, _ta_bump) =
            Pubkey::find_program_address(&[merkle_tree.as_ref()], &bubblegum_cpi::id());
        tree_authority
    }

    pub fn bubblegum_signer(&self) -> Pubkey {
        let (bubblegum_signer, _bs_bump) =
            Pubkey::find_program_address(&[b"collection_cpi"], &bubblegum_cpi::id());
        bubblegum_signer
    }

    pub fn entity_creator_key(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"entity_creator", self.key().as_ref()],
            &helium_entity_manager::id(),
        );
        key
    }

    pub fn key_to_asset(&self, entity_key: &[u8]) -> Pubkey {
        let hash = Sha256::digest(entity_key);
        let (key, _) = Pubkey::find_program_address(
            &[b"key_to_asset", self.key().as_ref(), hash.as_ref()],
            &helium_entity_manager::id(),
        );
        key
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubDao {
    Iot,
    Mobile,
}

impl std::fmt::Display for SubDao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Self::Iot => "iot",
            Self::Mobile => "mobile",
        };
        f.write_str(str)
    }
}

impl SubDao {
    pub const fn all() -> [SubDao; 2] {
        [SubDao::Iot, SubDao::Mobile]
    }

    pub fn key(&self) -> Pubkey {
        let mint = self.mint();
        let (subdao_key, _) =
            Pubkey::find_program_address(&[b"sub_dao", mint.as_ref()], &helium_sub_daos::id());
        subdao_key
    }

    pub fn mint(&self) -> &Pubkey {
        match self {
            Self::Iot => Token::Iot.mint(),
            Self::Mobile => Token::Mobile.mint(),
        }
    }

    pub fn dc_key() -> Pubkey {
        let (key, _) =
            Pubkey::find_program_address(&[b"dc", Token::Dc.mint().as_ref()], &data_credits::id());
        key
    }

    pub fn delegated_dc_key(&self, router_key: &str) -> Pubkey {
        let hash = Sha256::digest(router_key);
        let (key, _) = Pubkey::find_program_address(
            &[b"delegated_data_credits", self.key().as_ref(), &hash],
            &data_credits::id(),
        );
        key
    }

    pub fn escrow_account_key(&self, delegated_dc_key: &Pubkey) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"escrow_dc_account", delegated_dc_key.as_ref()],
            &data_credits::id(),
        );
        key
    }

    pub fn rewardable_entity_config_key(&self) -> Pubkey {
        let suffix = match self {
            Self::Iot => b"IOT".as_ref(),
            Self::Mobile => b"MOBILE".as_ref(),
        };
        let (key, _) = Pubkey::find_program_address(
            &[b"rewardable_entity_config", self.key().as_ref(), suffix],
            &helium_entity_manager::id(),
        );
        key
    }

    pub fn info_key(&self, entity_key: &[u8]) -> Pubkey {
        let hash = Sha256::digest(entity_key);
        let config_key = self.rewardable_entity_config_key();
        let prefix = match self {
            Self::Iot => "iot_info",
            Self::Mobile => "mobile_info",
        };
        let (key, _) = Pubkey::find_program_address(
            &[prefix.as_bytes(), config_key.as_ref(), &hash],
            &helium_entity_manager::id(),
        );
        key
    }

    pub fn info_key_for_helium_key(&self, public_key: &helium_crypto::PublicKey) -> Result<Pubkey> {
        let entity_key = hotspot::hotspot_key_to_entity(public_key)?;
        Ok(self.info_key(&entity_key))
    }

    pub fn lazy_distributor_key(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"lazy_distributor", self.mint().as_ref()],
            &lazy_distributor::id(),
        );
        key
    }
}
