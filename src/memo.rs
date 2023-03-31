use crate::{
    b64,
    result::{anyhow, Result},
};
use serde::de::{self, Deserialize, Deserializer, Visitor};
use std::{fmt, str::FromStr};

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Memo(u64);

impl FromStr for Memo {
    type Err = crate::result::Error;

    fn from_str(s: &str) -> Result<Self> {
        match b64::decode_u64(s) {
            Ok(v) => Ok(Memo(v)),
            Err(_) => Err(anyhow!("Invalid base64 memo")),
        }
    }
}

impl From<u64> for Memo {
    fn from(v: u64) -> Self {
        Memo(v)
    }
}

impl From<&Memo> for u64 {
    fn from(v: &Memo) -> Self {
        v.0
    }
}

impl fmt::Display for Memo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&b64::encode_u64(self.0))
    }
}

impl<'de> Deserialize<'de> for Memo {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MemoVisitor;

        impl<'de> Visitor<'de> for MemoVisitor {
            type Value = Memo;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("base64 string")
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Memo, E>
            where
                E: de::Error,
            {
                match b64::decode_u64(value) {
                    Ok(v) => Ok(Memo(v)),
                    Err(_) => Err(de::Error::custom("invalid memo")),
                }
            }
        }

        deserializer.deserialize_str(MemoVisitor)
    }
}
