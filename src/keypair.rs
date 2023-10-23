use crate::{
    mnemonic,
    result::{anyhow, Result},
    solana_sdk::{
        self,
        signature::{Signer, SignerError},
    },
    traits::ReadWrite,
};
use std::{io, rc::Rc};

#[derive(PartialEq, Debug)]
pub struct Keypair(solana_sdk::signer::keypair::Keypair);
pub struct VoidKeypair;

pub use solana_sdk::pubkey::Pubkey;

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
            Ok(Pubkey::try_from(&bytes[1..])?)
        }
        _ => anyhow::bail!("unsupported key type"),
    }
}

pub fn to_helium_pubkey(key: &Pubkey) -> Result<helium_crypto::PublicKey> {
    use helium_crypto::ReadFrom;
    let mut input = std::io::Cursor::new(key.as_ref());
    let helium_key = helium_crypto::ed25519::PublicKey::read_from(&mut input)?;
    Ok(helium_key.into())
}

static START: std::sync::Once = std::sync::Once::new();

fn init() {
    START.call_once(|| sodiumoxide::init().expect("Failed to intialize sodium"))
}

impl Default for Keypair {
    fn default() -> Self {
        Self::generate()
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

impl PublicKey for Rc<Keypair> {
    fn public_key(&self) -> solana_sdk::pubkey::Pubkey {
        self.0.pubkey()
    }
}

impl Keypair {
    pub fn generate() -> Self {
        Keypair(solana_sdk::signer::keypair::Keypair::new())
    }

    pub fn void() -> Rc<VoidKeypair> {
        Rc::new(VoidKeypair)
    }

    pub fn generate_from_entropy(entropy: &[u8]) -> Result<Self> {
        Ok(Keypair(
            solana_sdk::signer::keypair::keypair_from_seed(entropy)
                .map_err(|e| anyhow!("Failed to generate keypair: {e}"))?,
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
    pub fn phrase(&self) -> Result<String> {
        let words = mnemonic::entropy_to_mnemonic(self.0.secret().as_bytes())?;
        Ok(words.join(" "))
    }

    pub fn from_words(words: Vec<String>) -> Result<Rc<Self>> {
        let entropy_bytes = mnemonic::mnemonic_to_entropy(words)?;
        let keypair = solana_sdk::signer::keypair::keypair_from_seed(&entropy_bytes)
            .map_err(|e| anyhow!("failed to create keypair: {e}"))?;
        Ok(Self(keypair).into())
    }
}

impl ReadWrite for Keypair {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&self.0.to_bytes())?;
        Ok(())
    }

    fn read(reader: &mut dyn io::Read) -> Result<Keypair> {
        init();
        let mut sk_buf = [0u8; 64];
        reader.read_exact(&mut sk_buf)?;
        Ok(Self(solana_sdk::signer::keypair::Keypair::from_bytes(
            &sk_buf,
        )?))
    }
}

impl Signer for Keypair {
    fn try_pubkey(&self) -> std::result::Result<Pubkey, SignerError> {
        self.0.try_pubkey()
    }

    fn try_sign_message(
        &self,
        message: &[u8],
    ) -> std::result::Result<solana_sdk::signature::Signature, SignerError> {
        self.0.try_sign_message(message)
    }

    fn is_interactive(&self) -> bool {
        self.0.is_interactive()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, str::FromStr};

    #[test]
    fn roundtrip_keypair() {
        let keypair = Keypair::default();
        let mut buffer = Vec::new();
        keypair
            .write(&mut buffer)
            .expect("Failed to encode keypair");

        let decoded = Keypair::read(&mut Cursor::new(buffer)).expect("Failed to decode keypair");
        assert_eq!(keypair, decoded);
    }

    #[test]
    fn roundtrip_public_key() {
        let pk = Keypair::default();
        let mut buffer = Vec::new();
        pk.public_key()
            .write(&mut buffer)
            .expect("Failed to encode public key");

        let decoded = Pubkey::read(&mut Cursor::new(buffer)).expect("Failed to decode public key");
        assert_eq!(pk.public_key(), decoded);
    }

    #[test]
    fn roundtrip_b58_public_key() {
        let pk = Keypair::default();
        let decoded =
            Pubkey::from_str(&pk.public_key().to_string()).expect("Failed to decode public key");
        assert_eq!(pk.public_key(), decoded);
    }

    #[test]
    fn test_seed_output() {
        let pk = Keypair::default();
        let seed = pk.0.to_bytes();
        assert_eq!(64, seed.len());
    }
}
