use std::fmt::Display;

use crate::error::DecodeError;
use solana_sdk::bs58;

pub trait AsEntityKey {
    fn as_entity_key(&self) -> Vec<u8>;
}

impl AsEntityKey for String {
    fn as_entity_key(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl AsEntityKey for &str {
    fn as_entity_key(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl AsEntityKey for &[u8] {
    fn as_entity_key(&self) -> Vec<u8> {
        self.to_vec()
    }
}

impl AsEntityKey for Vec<u8> {
    fn as_entity_key(&self) -> Vec<u8> {
        self.clone()
    }
}

impl AsEntityKey for helium_crypto::PublicKey {
    fn as_entity_key(&self) -> Vec<u8> {
        // Entity keys are (regrettably) encoded through the bytes of the b58
        // string form of the helium public key
        bs58::decode(self.to_string()).into_vec().unwrap() // Safe to unwrap
    }
}

pub use crate::helium_entity_manager::KeySerialization;

pub fn from_str(str: &str, encoding: KeySerialization) -> Result<Vec<u8>, DecodeError> {
    let entity_key = match encoding {
        KeySerialization::UTF8 => str.as_entity_key(),
        KeySerialization::B58 => bs58::decode(str)
            .into_vec()
            .map_err(|_| DecodeError::other(format!("invalid entity key {}", str)))?,
    };
    Ok(entity_key)
}

#[derive(Debug, Clone, serde::Serialize, Copy, Default)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum EntityKeyEncoding {
    #[default]
    B58,
    UTF8,
}

impl Display for EntityKeyEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UTF8 => f.write_str("utf8"),
            Self::B58 => f.write_str("b58"),
        }
    }
}

impl From<EntityKeyEncoding> for KeySerialization {
    fn from(value: EntityKeyEncoding) -> Self {
        match value {
            EntityKeyEncoding::B58 => KeySerialization::B58,
            EntityKeyEncoding::UTF8 => KeySerialization::UTF8,
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct EncodedEntityKey {
    #[cfg_attr(feature="clap", clap(long, default_value_t = EntityKeyEncoding::UTF8))]
    pub encoding: EntityKeyEncoding,
    pub entity_key: String,
}

impl EncodedEntityKey {
    pub fn as_entity_key(&self) -> Result<Vec<u8>, DecodeError> {
        from_str(&self.entity_key, self.encoding.into())
    }
}

impl Display for EncodedEntityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.entity_key)
    }
}

impl From<&helium_crypto::PublicKey> for EncodedEntityKey {
    fn from(value: &helium_crypto::PublicKey) -> Self {
        Self {
            encoding: EntityKeyEncoding::B58,
            entity_key: value.to_string(),
        }
    }
}
