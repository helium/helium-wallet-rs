use crate::{
    result::{anyhow, Result},
    traits::B64,
};
use std::{fmt, str::FromStr};

#[derive(Debug, Default, PartialEq)]
pub struct Memo(u64);

impl FromStr for Memo {
    type Err = crate::result::Error;

    fn from_str(s: &str) -> Result<Self> {
        match u64::from_b64(s) {
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

impl AsRef<u64> for Memo {
    fn as_ref(&self) -> &u64 {
        &self.0
    }
}

impl fmt::Display for Memo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_b64().unwrap())
    }
}
