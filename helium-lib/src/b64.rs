use crate::result::{DecodeError, EncodeError, Result};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use helium_proto::Message;

pub fn encode<T: AsRef<[u8]>>(v: T) -> String {
    STANDARD.encode(v.as_ref())
}

pub fn encode_message<T: Message>(v: &T) -> Result<String> {
    let mut buf = vec![];
    v.encode(&mut buf).map_err(EncodeError::from)?;
    Ok(STANDARD.encode(buf))
}

pub fn decode_message<T>(v: &str) -> Result<T>
where
    T: Message + Default,
{
    let decoded = STANDARD.decode(v).map_err(DecodeError::from)?;
    let message = T::decode(&decoded[..]).map_err(DecodeError::from)?;
    Ok(message)
}

pub fn url_encode<T: AsRef<[u8]>>(v: T) -> String {
    URL_SAFE_NO_PAD.encode(v.as_ref())
}

pub fn decode<T: AsRef<[u8]>>(v: T) -> Result<Vec<u8>> {
    Ok(STANDARD.decode(v.as_ref()).map_err(DecodeError::from)?)
}

pub fn url_decode<T: AsRef<[u8]>>(v: T) -> Result<Vec<u8>> {
    Ok(URL_SAFE_NO_PAD
        .decode(v.as_ref())
        .map_err(DecodeError::from)?)
}

pub fn encode_u64(v: u64) -> String {
    STANDARD.encode(v.to_le_bytes())
}

pub fn decode_u64(v: &str) -> Result<u64> {
    let decoded = STANDARD.decode(v).map_err(DecodeError::from)?;
    let int_bytes = decoded.as_slice().try_into().map_err(DecodeError::from)?;
    Ok(u64::from_le_bytes(int_bytes))
}
