use crate::result::Result;
use helium_api::{BlockchainTxn, Message};
use std::convert::TryInto;

pub trait B64 {
    fn to_b64(&self) -> Result<String>;
    fn from_b64(str: &str) -> Result<Self>
    where
        Self: std::marker::Sized;
}

impl B64 for BlockchainTxn {
    fn to_b64(&self) -> Result<String> {
        let mut buf = vec![];
        self.encode(&mut buf)?;
        Ok(base64::encode(&buf))
    }

    fn from_b64(b64: &str) -> Result<Self> {
        let decoded = base64::decode(b64)?;
        let envelope = BlockchainTxn::decode(&decoded[..])?;
        Ok(envelope)
    }
}

impl B64 for u64 {
    fn to_b64(&self) -> Result<String> {
        Ok(base64::encode(&self.to_le_bytes()))
    }

    fn from_b64(b64: &str) -> Result<Self> {
        let decoded = base64::decode(b64)?;
        let int_bytes = decoded.as_slice().try_into()?;
        Ok(Self::from_le_bytes(int_bytes))
    }
}
