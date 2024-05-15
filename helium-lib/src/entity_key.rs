use crate::result::{DecodeError, Result};
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
        // Entity keys are (regrettably) encoded through the bytes of a the b58
        // string form of the helium public key
        bs58::decode(self.to_string()).into_vec().unwrap() // Safe to unwrap
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum EntityKeyEncoding {
    String,
    UTF8,
}

pub fn from_string(str: String, encoding: EntityKeyEncoding) -> Result<Vec<u8>> {
    let entity_key = match encoding {
        EntityKeyEncoding::String => str.as_entity_key(),
        EntityKeyEncoding::UTF8 => bs58::decode(str).into_vec().map_err(DecodeError::from)?,
    };
    Ok(entity_key)
}
