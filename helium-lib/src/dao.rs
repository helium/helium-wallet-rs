use crate::{
    data_credits, entity_key::AsEntityKey, helium_entity_manager, helium_sub_daos, keypair::Pubkey,
    rewards_oracle, token::Token,
};
use sha2::{Digest, Sha256};

#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum Dao {
    #[default]
    Hnt,
}

impl std::fmt::Display for Dao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("hnt")
    }
}

impl Dao {
    pub fn key(&self) -> Pubkey {
        let mint = match self {
            Self::Hnt => Token::Hnt.mint(),
        };
        let (dao_key, _) =
            Pubkey::find_program_address(&[b"dao", mint.as_ref()], &helium_sub_daos::ID);
        dao_key
    }

    pub fn dataonly_config_key(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"data_only_config", self.key().as_ref()],
            &helium_entity_manager::ID,
        );
        key
    }

    pub fn dataonly_escrow_key(&self) -> Pubkey {
        let (data_only_escrow, _doe_bump) = Pubkey::find_program_address(
            &[b"data_only_escrow", self.dataonly_config_key().as_ref()],
            &helium_entity_manager::ID,
        );
        data_only_escrow
    }

    pub fn entity_creator_key(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"entity_creator", self.key().as_ref()],
            &helium_entity_manager::ID,
        );
        key
    }

    pub fn entity_key_to_kta_key<E: AsEntityKey + ?Sized>(&self, entity_key: &E) -> Pubkey {
        let hash = Sha256::digest(entity_key.as_entity_key());
        let (key, _) = Pubkey::find_program_address(
            &[b"key_to_asset", self.key().as_ref(), hash.as_ref()],
            &helium_entity_manager::ID,
        );
        key
    }

    pub fn oracle_signer_key() -> Pubkey {
        let (key, _) = Pubkey::find_program_address(&[b"oracle_signer"], &rewards_oracle::ID);
        key
    }

    pub fn dc_account_payer() -> Pubkey {
        let (key, _) = Pubkey::find_program_address(&[b"account_payer"], &data_credits::ID);
        key
    }

    pub fn dc_key() -> Pubkey {
        let (key, _) =
            Pubkey::find_program_address(&[b"dc", Token::Dc.mint().as_ref()], &data_credits::ID);
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
        let (subdao_key, _) = Pubkey::find_program_address(
            &[b"sub_dao", self.token().mint().as_ref()],
            &helium_sub_daos::ID,
        );
        subdao_key
    }

    pub fn token(&self) -> Token {
        match self {
            Self::Iot => Token::Iot,
            Self::Mobile => Token::Mobile,
        }
    }

    pub fn delegated_dc_key<E: AsEntityKey>(&self, router_key: &E) -> Pubkey {
        let hash = Sha256::digest(router_key.as_entity_key());
        let (key, _) = Pubkey::find_program_address(
            &[b"delegated_data_credits", self.key().as_ref(), &hash],
            &data_credits::ID,
        );
        key
    }

    pub fn escrow_key(&self, delegated_dc_key: &Pubkey) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"escrow_dc_account", delegated_dc_key.as_ref()],
            &data_credits::ID,
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
            &helium_entity_manager::ID,
        );
        key
    }

    pub fn info_key<E: AsEntityKey>(&self, entity_key: &E) -> Pubkey {
        let hash = Sha256::digest(entity_key.as_entity_key());
        let config_key = self.rewardable_entity_config_key();
        let prefix = match self {
            Self::Iot => "iot_info",
            Self::Mobile => "mobile_info",
        };
        let (key, _) = Pubkey::find_program_address(
            &[prefix.as_bytes(), config_key.as_ref(), &hash],
            &helium_entity_manager::ID,
        );
        key
    }

    pub fn config_key(&self) -> Pubkey {
        let prefix = match self {
            Self::Iot => "iot_config",
            Self::Mobile => "mobile_config",
        };
        let (key, _) = Pubkey::find_program_address(
            &[prefix.as_bytes(), self.key().as_ref()],
            &helium_entity_manager::ID,
        );
        key
    }

    pub fn epoch_info_key(&self) -> Pubkey {
        const EPOCH_LENGTH: i64 = 60 * 60 * 24;
        let epoch = chrono::Utc::now().timestamp() / EPOCH_LENGTH;

        let (key, _) = Pubkey::find_program_address(
            &[
                "sub_dao_epoch_info".as_bytes(),
                self.key().as_ref(),
                &epoch.to_le_bytes(),
            ],
            &helium_sub_daos::ID,
        );
        key
    }
}
