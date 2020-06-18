use crate::{
    keypair::{PubKeyBin, PublicKey, KEYTYPE_ED25519},
    result::Result,
};
use io::{Read, Write};
use std::io;

pub trait ReadWrite {
    fn read(reader: &mut dyn Read) -> Result<Self>
    where
        Self: std::marker::Sized;
    fn write(&self, writer: &mut dyn Write) -> Result;
}

impl ReadWrite for PublicKey {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        let pubkey_bin: PubKeyBin = self.into();
        pubkey_bin.write(writer)
    }

    fn read(reader: &mut dyn Read) -> Result<PublicKey> {
        let pubkey_bin = PubKeyBin::read(reader)?;
        if pubkey_bin.0[0] != KEYTYPE_ED25519 {
            return Err(format!("Invalid key type {}", pubkey_bin.0[0]).into());
        }
        let pubkey: PublicKey = pubkey_bin.into();
        Ok(pubkey)
    }
}

impl ReadWrite for PubKeyBin {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&self.0)?;
        Ok(())
    }

    fn read(reader: &mut dyn Read) -> Result<Self> {
        let mut pubkey_bin = PubKeyBin::default();
        reader.read_exact(&mut pubkey_bin.0)?;
        Ok(pubkey_bin)
    }
}
