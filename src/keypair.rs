use crate::{result::Result, traits::ReadWrite};
use byteorder::ReadBytesExt;
use std::{convert::TryFrom, io};

use helium_crypto::{ecc_compact, ed25519};
pub use helium_crypto::{
    KeyTag, KeyType, Network, PublicKey, Sign, Verify, KEYTYPE_ED25519_STR, NETTYPE_MAIN_STR,
    PUBLIC_KEY_LENGTH,
};

#[derive(Debug, PartialEq)]
pub enum Keypair {
    Ed25519(helium_crypto::ed25519::Keypair),
    EccCompact(helium_crypto::ecc_compact::Keypair),
}

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
        match key_tag.key_type {
            KeyType::Ed25519 => {
                Self::Ed25519(ed25519::Keypair::generate(key_tag.network, &mut OsRng))
            }
            KeyType::EccCompact => {
                Self::EccCompact(ecc_compact::Keypair::generate(key_tag.network, &mut OsRng))
            }
        }
    }

    pub fn generate_from_entropy(key_tag: KeyTag, entropy: &[u8]) -> Result<Self> {
        match key_tag.key_type {
            KeyType::Ed25519 => Ok(Self::Ed25519(ed25519::Keypair::generate_from_entropy(
                key_tag.network,
                entropy,
            )?)),
            KeyType::EccCompact => Ok(Self::EccCompact(
                ecc_compact::Keypair::generate_from_entropy(key_tag.network, entropy)?,
            )),
        }
    }

    pub fn public_key(&self) -> &PublicKey {
        match self {
            Self::Ed25519(key) => &key.public_key,
            Self::EccCompact(key) => &key.public_key,
        }
    }

    pub fn sign(&self, msg: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::Ed25519(key) => Ok(key.sign(msg)?),
            Self::EccCompact(key) => Ok(key.sign(msg)?),
        }
    }
}

impl ReadWrite for Keypair {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        match self {
            Self::Ed25519(key) => {
                writer.write_all(&key.to_bytes())?;
                writer.write_all(&key.public_key.to_bytes())?;
            }
            Self::EccCompact(key) => {
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
                Ok(Keypair::Ed25519(ed25519::Keypair::try_from(&sk_buf[..])?))
            }
            KeyType::EccCompact => {
                let mut sk_buf = [0u8; ecc_compact::KEYPAIR_LENGTH];
                sk_buf[0] = tag;
                reader.read_exact(&mut sk_buf[1..])?;
                Ok(Keypair::EccCompact(ecc_compact::Keypair::try_from(
                    &sk_buf[..],
                )?))
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
