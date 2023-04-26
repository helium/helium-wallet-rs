use crate::{
    result::{bail, Result},
    solana_sdk,
};
use helium_crypto::{ecc_compact, ed25519, multisig, KeyType};
use io::{Read, Write};
use std::{convert::TryFrom, io};

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
        };
        data.resize(key_size, 0);
        reader.read_exact(&mut data[1..])?;
        Ok(helium_crypto::PublicKey::from_bytes(data)?)
    }
}

impl ReadWrite for solana_sdk::pubkey::Pubkey {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&self.to_bytes())?;
        Ok(())
    }

    fn read(reader: &mut dyn Read) -> Result<solana_sdk::pubkey::Pubkey> {
        let mut data = [0u8; solana_sdk::pubkey::PUBKEY_BYTES];
        reader.read_exact(&mut data)?;
        Ok(Self::new_from_array(data))
    }
}
