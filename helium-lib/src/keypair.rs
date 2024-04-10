use crate::result::{DecodeError, Result};
use solana_sdk::signature::{Signer, SignerError};
use std::sync::Arc;

#[derive(PartialEq, Debug)]
pub struct Keypair(solana_sdk::signer::keypair::Keypair);
pub struct VoidKeypair;

pub use solana_sdk::pubkey;
pub use solana_sdk::{pubkey::Pubkey, pubkey::PUBKEY_BYTES, signature::Signature};

pub mod serde_pubkey {
    use super::*;
    use serde::de::{self, Deserialize};
    use std::str::FromStr;

    pub fn serialize<S>(value: &Pubkey, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deser: D) -> std::result::Result<Pubkey, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let str = String::deserialize(deser)?;
        Pubkey::from_str(&str).map_err(|_| de::Error::custom("invalid public key"))
    }
}

pub mod serde_opt_pubkey {
    use super::*;
    use serde::{Deserialize, Serialize};

    pub fn serialize<S>(
        value: &Option<Pubkey>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Helper<'a>(#[serde(with = "serde_pubkey")] &'a Pubkey);
        value.as_ref().map(Helper).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<Option<Pubkey>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper(#[serde(with = "serde_pubkey")] Pubkey);
        let helper = Option::deserialize(deserializer)?;
        Ok(helper.map(|Helper(external)| external))
    }
}

pub fn to_pubkey(key: &helium_crypto::PublicKey) -> Result<Pubkey> {
    match key.key_type() {
        helium_crypto::KeyType::Ed25519 => {
            let bytes = key.to_vec();
            Ok(Pubkey::try_from(&bytes[1..]).map_err(DecodeError::from)?)
        }
        _ => Err(DecodeError::other("unsupported key type").into()),
    }
}

pub fn to_helium_pubkey(key: &Pubkey) -> Result<helium_crypto::PublicKey> {
    use helium_crypto::ReadFrom;
    let mut input = std::io::Cursor::new(key.as_ref());
    let helium_key = helium_crypto::ed25519::PublicKey::read_from(&mut input)?;
    Ok(helium_key.into())
}

impl std::ops::Deref for Keypair {
    type Target = solana_sdk::signer::keypair::Keypair;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for Keypair {
    fn default() -> Self {
        Self::generate()
    }
}

impl TryFrom<&[u8; 64]> for Keypair {
    type Error = DecodeError;
    fn try_from(value: &[u8; 64]) -> std::result::Result<Self, Self::Error> {
        solana_sdk::signer::keypair::Keypair::from_bytes(value)
            .map_err(|_| DecodeError::other("invalid keypair data"))
            .map(Self)
    }
}

pub trait PublicKey {
    fn public_key(&self) -> solana_sdk::pubkey::Pubkey;
}

impl PublicKey for Keypair {
    fn public_key(&self) -> solana_sdk::pubkey::Pubkey {
        self.0.pubkey()
    }
}

impl PublicKey for Arc<Keypair> {
    fn public_key(&self) -> solana_sdk::pubkey::Pubkey {
        self.0.pubkey()
    }
}

impl Keypair {
    pub fn generate() -> Self {
        Keypair(solana_sdk::signer::keypair::Keypair::new())
    }

    pub fn void() -> Arc<VoidKeypair> {
        Arc::new(VoidKeypair)
    }

    pub fn generate_from_entropy(entropy: &[u8]) -> Result<Self> {
        Ok(Keypair(
            solana_sdk::signer::keypair::keypair_from_seed(entropy)
                .map_err(|e| DecodeError::other(format!("invalid entropy: {e}")))?,
        ))
    }

    pub fn secret(&self) -> Vec<u8> {
        let mut result = self.0.secret().to_bytes().to_vec();
        result.extend_from_slice(self.public_key().as_ref());
        result
    }

    pub fn sign(&self, msg: &[u8]) -> Result<solana_sdk::signature::Signature> {
        Ok(self.try_sign_message(msg)?)
    }

    /// Return the mnemonic phrase that can be used to recreate this Keypair.
    /// This function is implemented here to avoid passing the secret between
    /// too many modules.
    #[cfg(feature = "mnemonic")]
    pub fn phrase(&self) -> Result<String> {
        let words = helium_mnemonic::entropy_to_mnemonic(self.0.secret().as_bytes())?;
        Ok(words.join(" "))
    }

    #[cfg(feature = "mnemonic")]
    pub fn from_words(words: Vec<String>) -> Result<Arc<Self>> {
        let entropy_bytes = helium_mnemonic::mnemonic_to_entropy(words)?;
        let keypair = solana_sdk::signer::keypair::keypair_from_seed(&entropy_bytes)
            .map_err(|_| DecodeError::other("invalid words"))?;
        Ok(Self(keypair).into())
    }
}

impl VoidKeypair {
    pub fn sign(&self, msg: &[u8]) -> Result<solana_sdk::signature::Signature> {
        Ok(self.try_sign_message(msg)?)
    }

    fn void_signer_error() -> SignerError {
        SignerError::Custom("Void Keypair".to_string())
    }
}

impl Signer for VoidKeypair {
    fn try_pubkey(&self) -> std::result::Result<Pubkey, SignerError> {
        Err(Self::void_signer_error())
    }

    fn try_sign_message(
        &self,
        _message: &[u8],
    ) -> std::result::Result<solana_sdk::signature::Signature, SignerError> {
        Err(Self::void_signer_error())
    }

    fn is_interactive(&self) -> bool {
        false
    }
}
