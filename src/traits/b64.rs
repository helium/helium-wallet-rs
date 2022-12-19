use crate::result::Result;
use helium_proto::{BlockchainTxn, Message};
use std::convert::TryInto;

const URL_SAFE_ENGINE: base64::engine::fast_portable::FastPortable =
    base64::engine::fast_portable::FastPortable::from(
        &base64::alphabet::URL_SAFE,
        base64::engine::fast_portable::NO_PAD,
    );

pub trait B64 {
    fn to_b64(&self) -> Result<String> {
        self.to_b64_config(&base64::engine::DEFAULT_ENGINE)
    }
    fn to_b64_url(&self) -> Result<String> {
        self.to_b64_config(&URL_SAFE_ENGINE)
    }
    fn from_b64(str: &str) -> Result<Self>
    where
        Self: Sized,
    {
        Self::from_b64_config(str, &base64::engine::DEFAULT_ENGINE)
    }
    fn from_b64_url(str: &str) -> Result<Self>
    where
        Self: Sized,
    {
        Self::from_b64_config(str, &URL_SAFE_ENGINE)
    }

    fn to_b64_config<E: base64::engine::Engine>(&self, config: &E) -> Result<String>;
    fn from_b64_config<E: base64::engine::Engine>(str: &str, config: &E) -> Result<Self>
    where
        Self: Sized;
}

impl B64 for BlockchainTxn {
    fn to_b64_config<E: base64::engine::Engine>(&self, config: &E) -> Result<String> {
        let mut buf = vec![];
        self.encode(&mut buf)?;
        Ok(base64::encode_engine(&buf, config))
    }

    fn from_b64_config<E: base64::engine::Engine>(b64: &str, config: &E) -> Result<Self> {
        let decoded = base64::decode_engine(b64, config)?;
        let envelope = BlockchainTxn::decode(&decoded[..])?;
        Ok(envelope)
    }
}

impl B64 for u64 {
    fn to_b64_config<E: base64::engine::Engine>(&self, config: &E) -> Result<String> {
        Ok(base64::encode_engine(self.to_le_bytes(), config))
    }

    fn from_b64_config<E: base64::engine::Engine>(b64: &str, config: &E) -> Result<Self> {
        let decoded = base64::decode_engine(b64, config)?;
        let int_bytes = decoded.as_slice().try_into()?;
        Ok(Self::from_le_bytes(int_bytes))
    }
}

impl B64 for Vec<u8> {
    fn to_b64_config<E: base64::engine::Engine>(&self, config: &E) -> Result<String> {
        Ok(base64::encode_engine(self, config))
    }

    fn from_b64_config<E: base64::engine::Engine>(b64: &str, config: &E) -> Result<Self> {
        let decoded = base64::decode_engine(b64, config)?;
        Ok(decoded)
    }
}
