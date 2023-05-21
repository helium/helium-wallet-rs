use crate::{
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

static START: std::sync::Once = std::sync::Once::new();

fn init() {
    START.call_once(|| sodiumoxide::init().expect("Failed to intialize sodium"))
}

impl Default for Keypair {
    fn default() -> Self {
        Self::generate()
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

    pub fn public_key(&self) -> solana_sdk::pubkey::Pubkey {
        self.0.pubkey()
    }

    pub fn sign(&self, msg: &[u8]) -> Result<solana_sdk::signature::Signature> {
        Ok(self.try_sign_message(msg)?)
    }

    /// Return the mnemonic phrase that can be used to recreate this Keypair.
    /// This function is implemented here to avoid passing the secret between
    /// too many modules.
    pub fn phrase(&self) -> Result<String> {
        use bip39::{Language, Mnemonic};
        let mnemonic = Mnemonic::from_entropy(self.0.secret().as_bytes(), Language::English)?;
        Ok(mnemonic.into_phrase())
    }

    pub fn from_phrase(phrase: &str) -> Result<Rc<Self>> {
        use bip39::{Language, Mnemonic};
        let mnemonic = Mnemonic::from_phrase(phrase, Language::English)?;
        let mut entropy_bytes = [0u8; 32];
        let mnemonic_entropy = mnemonic.entropy();
        if mnemonic_entropy.len() == 16 {
            entropy_bytes[..16].copy_from_slice(mnemonic_entropy);
            entropy_bytes[16..].copy_from_slice(mnemonic_entropy);
        } else {
            entropy_bytes.copy_from_slice(mnemonic_entropy)
        }
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
