use crate::{
    keypair::{PublicKey, PUBLIC_KEY_LENGTH},
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
        Ok(writer.write_all(&self.to_bytes())?)
    }

    fn read(reader: &mut dyn Read) -> Result<PublicKey> {
        let mut data = [0u8; PUBLIC_KEY_LENGTH];
        reader.read_exact(&mut data)?;
        Ok(PublicKey::from_bytes(data)?)
    }
}
