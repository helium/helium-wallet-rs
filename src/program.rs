use crate::keypair::Pubkey;
use std::str::FromStr;

lazy_static::lazy_static! {
    pub static ref HNT_PROGRAM_ID: Pubkey = Pubkey::from_str("hdaoVTCqhfHHo75XdAMxBKdUqvq1i5bF23sisBqVgGR").unwrap();
    pub static ref DC_PROGRAM_ID: Pubkey = Pubkey::from_str("credMBJhYFzfn7NxBMdU4aUqFggAjgztaCcv2Fo6fPT").unwrap();
    pub static ref HEM_PROGRAM_ID: Pubkey = Pubkey::from_str("hemjuPXBpNvggtaUnN1MwT3wrdhttKEfosTcc2P9Pg8").unwrap();
    pub static ref LAZY_DISTRIBUTOR_PROGRAM_ID: Pubkey = Pubkey::from_str("1azyuavdMyvsivtNxPoz6SucD18eDHeXzFCUPq5XU7w").unwrap();
}
