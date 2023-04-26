use crate::{keypair::Pubkey, result::Result, token::Token};
use sha2::{Digest, Sha256};
use std::str::FromStr;

lazy_static::lazy_static! {
    static ref HNT_PROGRAM_ID: Pubkey = Pubkey::from_str("hdaoVTCqhfHHo75XdAMxBKdUqvq1i5bF23sisBqVgGR").unwrap();
    static ref DC_PROGRAM_ID: Pubkey = Pubkey::from_str("credMBJhYFzfn7NxBMdU4aUqFggAjgztaCcv2Fo6fPT").unwrap();
    static ref HEM_PROGRAM_ID: Pubkey = Pubkey::from_str("hemjuPXBpNvggtaUnN1MwT3wrdhttKEfosTcc2P9Pg8").unwrap();
    static ref LAZY_DISTRIBUTOR_PROGRAM_ID: Pubkey = Pubkey::from_str("1azyuavdMyvsivtNxPoz6SucD18eDHeXzFCUPq5XU7w").unwrap();
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SubDao {
    Iot,
    Mobile,
}

impl std::fmt::Display for SubDao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&str)
    }
}

impl SubDao {
    pub const fn all() -> [SubDao; 2] {
        [SubDao::Iot, SubDao::Mobile]
    }

    pub fn key(&self) -> Pubkey {
        let mint = self.mint();
        let (subdao_key, _) =
            Pubkey::find_program_address(&[b"sub_dao", &mint.to_bytes()], &HNT_PROGRAM_ID);
        subdao_key
    }

    pub fn mint(&self) -> &Pubkey {
        match self {
            Self::Iot => Token::Iot.mint(),
            Self::Mobile => Token::Mobile.mint(),
        }
    }

    pub fn delegated_dc_key(&self, router_key: &str) -> Pubkey {
        let hash = Sha256::digest(router_key);
        let (key, _) = Pubkey::find_program_address(
            &[b"delegated_data_credits", &self.key().to_bytes(), &hash],
            &DC_PROGRAM_ID,
        );
        key
    }

    pub fn escrow_account_key(&self, delegated_dc_key: &Pubkey) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"escrow_dc_account", &delegated_dc_key.to_bytes()],
            &DC_PROGRAM_ID,
        );
        key
    }

    pub fn rewardable_entity_config_key(&self) -> Pubkey {
        let suffix = match self {
            Self::Iot => b"IOT".as_ref(),
            Self::Mobile => b"MOBILE".as_ref(),
        };
        let (key, _) = Pubkey::find_program_address(
            &[b"rewardable_entity_config", &self.key().to_bytes(), suffix],
            &HEM_PROGRAM_ID,
        );
        key
    }

    pub fn info_key(&self, entity_key: &helium_crypto::PublicKey) -> Result<Pubkey> {
        let config_key = self.rewardable_entity_config_key();
        let entity_decoded = bs58::decode(entity_key.to_string()).into_vec()?;
        let hash = Sha256::digest(entity_decoded);
        let prefix = match self {
            Self::Iot => "iot_info",
            Self::Mobile => "mobile_info",
        };
        let (key, _) = Pubkey::find_program_address(
            &[prefix.as_bytes(), &config_key.to_bytes(), &hash],
            &HEM_PROGRAM_ID,
        );
        Ok(key)
    }

    pub fn lazy_distributor_key(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"lazy_distributor", self.mint().as_ref()],
            &LAZY_DISTRIBUTOR_PROGRAM_ID,
        );
        key
    }
}
