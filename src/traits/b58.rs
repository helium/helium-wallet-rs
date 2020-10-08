use crate::{
    keypair::{PubKeyBin, PublicKey, KEYTYPE_ED25519},
    result::Result,
};

pub trait B58 {
    fn to_b58(&self) -> Result<String>;
    fn from_b58(str: &str) -> Result<Self>
    where
        Self: std::marker::Sized;
}

impl B58 for PublicKey {
    fn to_b58(&self) -> Result<String> {
        let pubkey_bin: PubKeyBin = self.into();
        pubkey_bin.to_b58()
    }

    fn from_b58(b58: &str) -> Result<PublicKey> {
        let pubkey_bin = PubKeyBin::from_b58(b58)?;

        if pubkey_bin.0[0] != KEYTYPE_ED25519 {
            return Err(format!("Invalid key type {}", pubkey_bin.0[0]).into());
        }
        let pubkey: PublicKey = pubkey_bin.into();
        Ok(pubkey)
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

    fn from_b58(b58: &str) -> Result<Self> {
        // First 0 value is the version byte
        let data = bs58::decode(b58).with_check(Some(0)).into_vec()?;
        let mut pubkey_bin = PubKeyBin::default();
        pubkey_bin.0.copy_from_slice(&data[1..]);
        Ok(pubkey_bin)
    }
}

impl B58 for Vec<u8> {
    fn to_b58(&self) -> Result<String> {
        Ok(bs58::encode(self).with_check().into_string())
    }

    fn from_b58(b58: &str) -> Result<Self> {
        Ok(bs58::decode(b58).with_check(Some(0)).into_vec()?)
    }
}
