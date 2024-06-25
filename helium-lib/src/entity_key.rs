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
        // Entity keys are (regrettably) encoded through the bytes of a the b58
        // string form of the helium public key
        bs58::decode(self.to_string()).into_vec().unwrap() // Safe to unwrap
    }
}

pub use helium_anchor_gen::helium_entity_manager::KeySerialization;

pub fn from_str(str: &str, encoding: KeySerialization) -> Result<Vec<u8>, DecodeError> {
    let entity_key = match encoding {
        KeySerialization::UTF8 => str.as_entity_key(),
        KeySerialization::B58 => bs58::decode(str)
            .into_vec()
            .map_err(|_| DecodeError::other(format!("invalid entity key {}", str)))?,
    };
    Ok(entity_key)
}
