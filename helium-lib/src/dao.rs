use crate::{
    data_credits, entity_key::AsEntityKey, helium_entity_manager, helium_sub_daos, keypair::Pubkey,
    lazy_distributor, metaplex, rewards_oracle, token::Token,
};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
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
        helium_sub_daos::dao_key(mint)
    }

    pub fn dataonly_config_key(&self) -> Pubkey {
        helium_entity_manager::data_only_config_key(&self.key())
    }

    pub fn dataonly_escrow_key(&self) -> Pubkey {
        helium_entity_manager::data_only_escrow_key(&self.dataonly_config_key())
    }

    pub fn collection_metadata_key(&self, collection_key: &Pubkey) -> Pubkey {
        metaplex::collection_metadata_key(collection_key)
    }

    pub fn collection_master_edition_key(&self, collection_key: &Pubkey) -> Pubkey {
        metaplex::collection_master_edition_key(collection_key)
    }

    pub fn merkle_tree_authority(&self, merkle_tree: &Pubkey) -> Pubkey {
        metaplex::merkle_tree_authority_key(merkle_tree)
    }

    pub fn bubblegum_signer(&self) -> Pubkey {
        metaplex::bubblegum_signer_key()
    }

    pub fn entity_creator_key(&self) -> Pubkey {
        helium_entity_manager::entity_creator_key(&self.key())
    }

    pub fn entity_key_to_kta_key<E: AsEntityKey + ?Sized>(&self, entity_key: &E) -> Pubkey {
        let hash = Sha256::digest(entity_key.as_entity_key());
        helium_entity_manager::key_to_asset_key_raw(&self.key(), &hash)
    }

    pub fn dc_account_payer() -> Pubkey {
        let (key, _) = Pubkey::find_program_address(&[b"account_payer"], &data_credits::id());
        key
    }

    pub fn dc_key() -> Pubkey {
        let (key, _) =
            Pubkey::find_program_address(&[b"dc", Token::Dc.mint().as_ref()], &data_credits::id());
        key
    }

    pub fn oracle_signer_key() -> Pubkey {
        let (key, _) = Pubkey::find_program_address(&[b"oracle_signer"], &rewards_oracle::id());
        key
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
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
        helium_sub_daos::sub_dao_key(mint)
    }

    pub fn mint(&self) -> &Pubkey {
        match self {
            Self::Iot => Token::Iot.mint(),
            Self::Mobile => Token::Mobile.mint(),
        }
    }

    pub fn token(&self) -> Token {
        match self {
            Self::Iot => Token::Iot,
            Self::Mobile => Token::Mobile,
        }
    }

    pub fn lazy_distributor(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"lazy_distributor", self.mint().as_ref()],
            &lazy_distributor::id(),
        );
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

    pub fn escrow_key(&self, delegated_dc_key: &Pubkey) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"escrow_dc_account", delegated_dc_key.as_ref()],
            &data_credits::id(),
        );
        key
    }

    pub fn rewardable_entity_config_key(&self) -> Pubkey {
        let sub_dao = self.key();
        match self {
            Self::Iot => helium_entity_manager::rewardable_entity_config_key(&sub_dao, "IOT"),
            Self::Mobile => helium_entity_manager::rewardable_entity_config_key(&sub_dao, "MOBILE"),
        }
    }

    pub fn info_key<E: AsEntityKey>(&self, entity_key: &E) -> Pubkey {
        let config_key = self.rewardable_entity_config_key();
        match self {
            Self::Iot => {
                helium_entity_manager::iot_info_key(&config_key, &entity_key.as_entity_key())
            }
            Self::Mobile => {
                helium_entity_manager::mobile_info_key(&config_key, &entity_key.as_entity_key())
            }
        }
    }

    pub fn lazy_distributor_key(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"lazy_distributor", self.mint().as_ref()],
            &lazy_distributor::id(),
        );
        key
    }

    pub fn receipient_key_from_kta(&self, kta: &helium_entity_manager::KeyToAssetV0) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[
                b"recipient",
                self.lazy_distributor_key().as_ref(),
                kta.asset.as_ref(),
            ],
            &lazy_distributor::id(),
        );
        key
    }

    pub fn config_key(&self) -> Pubkey {
        let sub_dao = self.key();
        match self {
            Self::Iot => helium_entity_manager::iot_config_key(&sub_dao),
            Self::Mobile => helium_entity_manager::mobile_config_key(&sub_dao),
        }
    }

    pub fn epoch_info_key(&self) -> Pubkey {
        let sub_dao = self.key();
        let unix_time = chrono::Utc::now().timestamp() as u64;
        helium_sub_daos::sub_dao_epoch_info_key(&sub_dao, unix_time)
    }
}
