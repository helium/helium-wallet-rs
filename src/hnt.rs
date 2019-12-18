use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::{error, fmt};

#[derive(Clone, Copy, Debug)]
pub struct Hnt {
    data: Decimal,
}

const HNT_TO_BONES_SCALAR: i32 = 100_000_000;

impl FromStr for Hnt {
    type Err = HntFromStrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let data = Decimal::from_str(s)
            .or_else(|_| Decimal::from_scientific(s))
            .unwrap();
        if data.scale() > 8 {
            Err(HntFromStrError { data })
        } else {
            Ok(Hnt { data })
        }
    }
}

impl Hnt {
    pub fn to_bones(&self) -> u64 {
        if let Some(scaled_dec) = self.data.checked_mul(HNT_TO_BONES_SCALAR.into()) {
            if let Some(num) = scaled_dec.to_u64() {
                return num;
            }
        }
        panic!("Hnt has been constructed with invalid data")
    }

    pub fn get_decimal(&self) -> Decimal {
        self.data
    }
}

#[derive(Debug)]
pub struct HntFromBonesError;
impl error::Error for HntFromBonesError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}
impl fmt::Display for HntFromBonesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unable to create Hnt from Bones (u64) input",)
    }
}

#[derive(Debug)]
pub struct HntFromStrError {
    data: Decimal,
}
impl fmt::Display for HntFromStrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
        	f,
        	"Attempted to build Hnt with string that has too many values after the decimal. {} values when only 8 is tolerated",
        	self.data.scale()
        )
    }
}

impl error::Error for HntFromStrError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}
