use crate::result::{bail, Result};
use helium_crypto::{ecc_compact, ed25519, multisig, KeyType};
use io::{Read, Write};
use std::io;

pub trait ReadWrite {
    fn read(reader: &mut dyn Read) -> Result<Self>
    where
        Self: std::marker::Sized;
    fn write(&self, writer: &mut dyn Write) -> Result;
}

impl ReadWrite for helium_crypto::PublicKey {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        Ok(writer.write_all(&self.to_vec())?)
    }

    fn read(reader: &mut dyn Read) -> Result<Self> {
        let mut data = vec![0u8; 1];
        reader.read_exact(&mut data[0..1])?;
        let key_size = match KeyType::try_from(data[0])? {
            KeyType::Ed25519 => ed25519::PUBLIC_KEY_LENGTH,
            KeyType::EccCompact => ecc_compact::PUBLIC_KEY_LENGTH,
            KeyType::MultiSig => multisig::PUBLIC_KEY_LENGTH,
            KeyType::Secp256k1 => bail!("Secp256k1 key type unsupported for read."),
            KeyType::Rsa => bail!("RSA key type unsupported for read."),
        };
        data.resize(key_size, 0);
        reader.read_exact(&mut data[1..])?;
        Ok(helium_crypto::PublicKey::from_bytes(data)?)
    }
}

impl ReadWrite for helium_lib::keypair::Pubkey {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&self.to_bytes())?;
        Ok(())
    }

    fn read(reader: &mut dyn Read) -> Result<helium_lib::keypair::Pubkey> {
        let mut data = [0u8; helium_lib::keypair::PUBKEY_BYTES];
        reader.read_exact(&mut data)?;
        Ok(Self::new_from_array(data))
    }
}

impl ReadWrite for helium_lib::keypair::Keypair {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&self.to_bytes())?;
        Ok(())
    }

    fn read(reader: &mut dyn io::Read) -> Result<Self> {
        let mut sk_buf = [0u8; 64];
        reader.read_exact(&mut sk_buf)?;
        Ok(Self::try_from(&sk_buf)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helium_lib::keypair::{Keypair, Pubkey, PublicKey};
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
        let seed = pk.to_bytes();
        assert_eq!(64, seed.len());
    }
}
