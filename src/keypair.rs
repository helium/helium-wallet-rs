use crate::{
    mnemonic::{entropy_to_mnemonic, SeedType},
    result::Result,
    traits::ReadWrite,
};
use byteorder::ReadBytesExt;
use std::{convert::TryFrom, io};

pub use helium_crypto::{
    ecc_compact, ed25519, KeyTag, KeyType, Network, PublicKey, Sign, Verify, KEYTYPE_ED25519_STR,
    NETTYPE_MAIN_STR, PUBLIC_KEY_LENGTH,
};

#[derive(PartialEq, Debug)]
pub struct Keypair(helium_crypto::Keypair);

static START: std::sync::Once = std::sync::Once::new();

fn init() {
    START.call_once(|| sodiumoxide::init().expect("Failed to intialize sodium"))
}

impl Default for Keypair {
    fn default() -> Self {
        Self::generate(KeyTag::default())
    }
}

impl Keypair {
    pub fn generate(key_tag: KeyTag) -> Self {
        use rand::rngs::OsRng;
        Keypair(helium_crypto::Keypair::generate(key_tag, &mut OsRng))
    }

    pub fn generate_from_entropy(key_tag: KeyTag, entropy: &[u8]) -> Result<Self> {
        Ok(Keypair(helium_crypto::Keypair::generate_from_entropy(
            key_tag, entropy,
        )?))
    }

    pub fn public_key(&self) -> &PublicKey {
        self.0.public_key()
    }

    pub fn sign(&self, msg: &[u8]) -> Result<Vec<u8>> {
        Ok(self.0.sign(msg)?)
    }

    /// Return the mnemonic phrase that can be used to recreate this Keypair.
    /// This function is implemented here to avoid passing the secret between
    /// too many modules.
    pub fn phrase(&self, seed_type: &SeedType) -> Result<Vec<String>> {
        let entropy: Vec<u8> = self.0.to_bytes()[1..33].to_vec();
        entropy_to_mnemonic(&entropy, seed_type)
    }
}

impl ReadWrite for Keypair {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        match &self.0 {
            helium_crypto::Keypair::Ed25519(key) => {
                writer.write_all(&key.to_bytes())?;
                writer.write_all(&key.public_key.to_bytes())?;
            }
            helium_crypto::Keypair::EccCompact(key) => {
                writer.write_all(&key.to_bytes())?;
                writer.write_all(&key.public_key.to_bytes())?;
            }
        }
        Ok(())
    }

    fn read(reader: &mut dyn io::Read) -> Result<Keypair> {
        init();
        let tag = reader.read_u8()?;
        match KeyType::try_from(tag)? {
            KeyType::Ed25519 => {
                let mut sk_buf = [0u8; ed25519::KEYPAIR_LENGTH];
                sk_buf[0] = tag;
                reader.read_exact(&mut sk_buf[1..])?;
                Ok(Keypair(ed25519::Keypair::try_from(&sk_buf[..])?.into()))
            }
            KeyType::EccCompact => {
                let mut sk_buf = [0u8; ecc_compact::KEYPAIR_LENGTH];
                sk_buf[0] = tag;
                reader.read_exact(&mut sk_buf[1..])?;
                Ok(Keypair(ecc_compact::Keypair::try_from(&sk_buf[..])?.into()))
            }
        }
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

        let decoded =
            PublicKey::read(&mut Cursor::new(buffer)).expect("Failed to decode public key");
        assert_eq!(pk.public_key(), &decoded);
    }

    #[test]
    fn roundtrip_b58_public_key() {
        let pk = Keypair::default();
        let decoded =
            PublicKey::from_str(&pk.public_key().to_string()).expect("Failed to decode public key");
        assert_eq!(pk.public_key(), &decoded);
    }
}
