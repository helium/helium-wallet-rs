use crate::error::{DecodeError, EncodeError};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use helium_proto::Message;

/// Base64-encode bytes using the standard alphabet.
pub fn encode<T: AsRef<[u8]>>(v: T) -> String {
    STANDARD.encode(v.as_ref())
}

/// Serialize a protobuf message and base64-encode the result.
pub fn encode_message<T: Message>(v: &T) -> Result<String, EncodeError> {
    let mut buf = vec![];
    v.encode(&mut buf).map_err(EncodeError::from)?;
    Ok(STANDARD.encode(buf))
}

/// Base64-decode a string and deserialize it as a protobuf message.
pub fn decode_message<T>(v: &str) -> Result<T, DecodeError>
where
    T: Message + Default,
{
    let decoded = STANDARD.decode(v).map_err(DecodeError::from)?;
    let message = T::decode(&decoded[..]).map_err(DecodeError::from)?;
    Ok(message)
}

/// Base64-encode bytes using the URL-safe alphabet without padding.
pub fn url_encode<T: AsRef<[u8]>>(v: T) -> String {
    URL_SAFE_NO_PAD.encode(v.as_ref())
}

/// Base64-decode bytes using the standard alphabet.
pub fn decode<T: AsRef<[u8]>>(v: T) -> Result<Vec<u8>, DecodeError> {
    STANDARD.decode(v.as_ref()).map_err(DecodeError::from)
}

/// Base64-decode bytes using the URL-safe alphabet without padding.
pub fn url_decode<T: AsRef<[u8]>>(v: T) -> Result<Vec<u8>, DecodeError> {
    URL_SAFE_NO_PAD
        .decode(v.as_ref())
        .map_err(DecodeError::from)
}

/// Base64-encode a `u64` as little-endian bytes.
pub fn encode_u64(v: u64) -> String {
    STANDARD.encode(v.to_le_bytes())
}

/// Base64-decode a string into a `u64` (little-endian).
pub fn decode_u64(v: &str) -> Result<u64, DecodeError> {
    let decoded = STANDARD.decode(v).map_err(DecodeError::from)?;
    let int_bytes = decoded.as_slice().try_into().map_err(DecodeError::from)?;
    Ok(u64::from_le_bytes(int_bytes))
}
