use crate::keypair::{Keypair, PubKeyBin, PublicKey, KEYTYPE_ED25519};
use crate::result::Result;
use bs58;
use helium_proto::{BlockchainTxnPaymentV1, Message};
use io::{Read, Write};
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

impl B58 for PublicKey {
    fn to_b58(&self) -> Result<String> {
        let pubkey_bin: PubKeyBin = self.into();
        pubkey_bin.to_b58()
    }

    fn from_b58(b58: String) -> Result<PublicKey> {
        let pubkey_bin = PubKeyBin::from_b58(b58)?;

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

impl B58 for PubKeyBin {
    fn to_b58(&self) -> Result<String> {
        // First 0 value is the "version" number defined for addresses
        // in libp2p
        let mut data = [0u8; 34];
        data[1..].copy_from_slice(&self.0);
        Ok(bs58::encode(data.as_ref()).with_check().into_string())
    }

    fn from_b58(b58: String) -> Result<Self> {
        // First 0 value is the version byte
        let data = bs58::decode(b58).with_check(Some(0)).into_vec()?;
        let mut pubkey_bin = PubKeyBin::default();
        pubkey_bin.0.copy_from_slice(&data[1..]);
        Ok(pubkey_bin)
    }
}

pub trait Sign {
    fn sign(&mut self, keypair: &Keypair) -> Result;
}

impl Sign for BlockchainTxnPaymentV1 {
    fn sign(&mut self, keypair: &Keypair) -> Result {
        let mut buf = vec![];
        self.encode(&mut buf)?;
        self.signature = keypair.sign(&buf);
        Ok(())
    }
}
