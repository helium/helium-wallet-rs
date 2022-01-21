use crate::{keypair::PublicKey, result::Result};
use helium_crypto::{ecc_compact, ed25519, multisig, KeyType};
use io::{Read, Write};
use std::{convert::TryFrom, io};

pub trait ReadWrite {
    fn read(reader: &mut dyn Read) -> Result<Self>
    where
        Self: std::marker::Sized;
    fn write(&self, writer: &mut dyn Write) -> Result;
}

impl ReadWrite for PublicKey {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        Ok(writer.write_all(&self.to_vec())?)
    }

    fn read(reader: &mut dyn Read) -> Result<PublicKey> {
        let mut data = vec![0u8; 1];
        reader.read_exact(&mut data[0..1])?;
        let key_size = match KeyType::try_from(data[0])? {
            KeyType::Ed25519 => ed25519::PUBLIC_KEY_LENGTH,
            KeyType::EccCompact => ecc_compact::PUBLIC_KEY_LENGTH,
            KeyType::MultiSig => multisig::PUBLIC_KEY_LENGTH,
        };
        data.resize(key_size, 0);
        reader.read_exact(&mut data[1..])?;
        Ok(PublicKey::from_bytes(data)?)
    }
}
