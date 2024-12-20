use crate::{
    data_credits, entity_key::AsEntityKey, get_current_epoch, helium_entity_manager,
    helium_sub_daos, keypair::Pubkey, lazy_distributor, metaplex, rewards_oracle, token::Token,
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

        Pubkey::find_program_address(&[b"dao", mint.as_ref()], &helium_sub_daos::ID).0
    }

    pub fn program_approval_key(&self, program: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[b"program_approval", &self.key().as_ref(), program.as_ref()],
            &helium_entity_manager::ID,
        )
        .0
    }

    pub fn dataonly_config_key(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[b"data_only_config", &self.key().as_ref()],
            &helium_entity_manager::ID,
        )
        .0
    }

    pub fn dataonly_escrow_key(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[b"data_only_escrow", &self.dataonly_config_key().as_ref()],
            &helium_entity_manager::ID,
        )
        .0
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
        Pubkey::find_program_address(
            &[b"entity_creator", &self.key().as_ref()],
            &helium_entity_manager::ID,
        )
        .0
    }

    pub fn entity_key_to_kta_key<E: AsEntityKey + ?Sized>(&self, entity_key: &E) -> Pubkey {
        let hash = Sha256::digest(entity_key.as_entity_key());

        Pubkey::find_program_address(
            &[b"key_to_asset", &self.key().as_ref(), &hash],
            &helium_entity_manager::ID,
        )
        .0
    }

    pub fn oracle_signer_key() -> Pubkey {
        let (key, _) = Pubkey::find_program_address(&[b"oracle_signer"], &rewards_oracle::id());
        key
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

    pub fn epoch_info_key(&self) -> Pubkey {
        let dao = self.key();
        let unix_time = chrono::Utc::now().timestamp() as u64;
        let epoch = get_current_epoch(unix_time);
        let b_u64 = epoch.to_le_bytes();
        Pubkey::find_program_address(
            &[b"dao_epoch_info", &dao.as_ref(), &b_u64],
            &helium_sub_daos::ID,
        )
        .0
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
        Pubkey::find_program_address(&[b"sub_dao", mint.as_ref()], &helium_sub_daos::ID).0
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
            Self::Iot => {
                Pubkey::find_program_address(
                    &[
                        b"rewardable_entity_config",
                        &sub_dao.as_ref(),
                        "IOT".as_bytes(),
                    ],
                    &helium_entity_manager::ID,
                )
                .0
            }
            Self::Mobile => {
                Pubkey::find_program_address(
                    &[
                        b"rewardable_entity_config",
                        &sub_dao.as_ref(),
                        "MOBILE".as_bytes(),
                    ],
                    &helium_entity_manager::ID,
                )
                .0
            }
        }
    }

    pub fn info_key<E: AsEntityKey>(&self, entity_key: &E) -> Pubkey {
        let config_key = self.rewardable_entity_config_key();
        let hash = Sha256::digest(&entity_key.as_entity_key());
        match self {
            Self::Iot => {
                Pubkey::find_program_address(
                    &[b"iot_info", &config_key.as_ref(), &hash],
                    &helium_entity_manager::ID,
                )
                .0
            }
            Self::Mobile => {
                Pubkey::find_program_address(
                    &[b"mobile_info", &config_key.as_ref(), &hash],
                    &helium_entity_manager::ID,
                )
                .0
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
            Self::Iot => {
                Pubkey::find_program_address(
                    &[b"iot_config", &sub_dao.as_ref()],
                    &helium_entity_manager::ID,
                )
                .0
            }
            Self::Mobile => {
                Pubkey::find_program_address(
                    &[b"mobile_config", sub_dao.as_ref()],
                    &helium_entity_manager::ID,
                )
                .0
            }
        }
    }

    pub fn epoch_info_key(&self) -> Pubkey {
        let sub_dao = self.key();
        let unix_time = chrono::Utc::now().timestamp() as u64;
        let epoch = get_current_epoch(unix_time);
        let b_u64 = epoch.to_le_bytes();
        Pubkey::find_program_address(
            &[b"sub_dao_epoch_info", &sub_dao.as_ref(), &b_u64],
            &helium_sub_daos::ID,
        )
        .0
    }
}
