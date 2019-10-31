use crate::keypair::KEYTYPE_ED25519;
use crate::result::Result;
use bs58;
use byteorder::ReadBytesExt;
use io::{Read, Write};
use sodiumoxide::crypto::sign::ed25519;
use std::io;

pub trait ReadWrite {
    fn read(reader: &mut dyn Read) -> Result<Self>
    where
        Self: std::marker::Sized;
    fn write(&self, writer: &mut dyn Write) -> Result;
}

pub trait B58 {
    fn to_b58(&self) -> Result<String>;
    fn from_b58(str: String) -> Result<Self>
    where
        Self: std::marker::Sized;
}

pub trait Empty {
    fn empty() -> Self;
}

impl ReadWrite for ed25519::PublicKey {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&[KEYTYPE_ED25519])?;
        writer.write_all(&self.0)?;
        Ok(())
    }

    fn read(reader: &mut dyn Read) -> Result<ed25519::PublicKey> {
        let key_type = reader.read_u8()?;
        if key_type != KEYTYPE_ED25519 {
            return Err(format!("Invalid public key type {}", key_type).into());
        }
        let mut pk_buf = [0; 32];
        reader.read_exact(&mut pk_buf)?;
        Ok(ed25519::PublicKey(pk_buf))
    }
}

impl B58 for ed25519::PublicKey {
    fn to_b58(&self) -> Result<String> {
        let mut payload = vec![0, KEYTYPE_ED25519];
        payload.write_all(&self.0)?;
        Ok(bs58::encode(payload).with_check().into_string())
    }

    fn from_b58(b58: String) -> Result<ed25519::PublicKey> {
        let binary = bs58::decode(b58).with_check(Some(0)).into_vec()?;
        if binary[1] != KEYTYPE_ED25519 {
            return Err(format!("Invalid key type {}", binary[1]).into());
        }

        let mut body = &binary[2..];
        let mut key_bytes: [u8; 32] = [0; 32];
        body.read_exact(&mut key_bytes)?;
        Ok(ed25519::PublicKey(key_bytes))
    }
}
