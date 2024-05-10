use std::{array::TryFromSliceError, num::TryFromIntError};
use thiserror::Error;

use crate::{onboarding, settings};

pub type Result<T = ()> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[cfg(feature = "mnemonic")]
    #[error("mnemonic: {0}")]
    Mnemonic(#[from] helium_mnemonic::MnmemonicError),
    #[error("onboarding: {0}")]
    Onboarding(#[from] onboarding::OnboardingError),
    #[error("anchor client: {0}")]
    Anchor(#[from] anchor_client::ClientError),
    #[error("anchor lang: {0}")]
    AnchorLang(#[from] helium_anchor_gen::anchor_lang::error::Error),
    #[error("DAS client: {0}")]
    Das(#[from] settings::DasClientError),
    #[error("pyth client: {0}")]
    Pyth(#[from] pyth_sdk_solana::PythError),
    #[error("rest client: {0}")]
    Rest(#[from] reqwest::Error),
    #[error("system time: {0}")]
    Time(#[from] std::time::SystemTimeError),
    #[error("program: {0}")]
    Program(#[from] solana_program::program_error::ProgramError),
    #[error("solana: {0}")]
    Solana(#[from] anchor_client::solana_client::client_error::ClientError),
    #[error("signing: {0}")]
    Signing(#[from] solana_sdk::signer::SignerError),
    #[error("crypto: {0}")]
    Crypto(#[from] helium_crypto::Error),
    #[error("decode: {0}")]
    Decode(#[from] DecodeError),
    #[error("encode: {0}")]
    Encode(#[from] EncodeError),
}

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("proto: {0}")]
    Proto(#[from] helium_proto::EncodeError),
    #[error("bincode: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("h3: {0}")]
    H3(#[from] h3o::error::InvalidLatLng),
    #[error("encode: {0}")]
    Encode(String),
}

impl EncodeError {
    pub fn other<S: ToString>(reason: S) -> Self {
        Self::Encode(reason.to_string())
    }
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("integer: {0}")]
    Int(#[from] TryFromIntError),
    #[error("url: {0}")]
    Url(#[from] url::ParseError), // decode
    #[error("hex: {0}")]
    Hex(#[from] hex::FromHexError), // decode
    #[error("base64: {0}")]
    Base64(#[from] base64::DecodeError), // decode
    #[error("proto: {0}")]
    Proto(#[from] helium_proto::DecodeError), // decode
    #[error("base58: {0}")]
    Bs58(#[from] solana_sdk::bs58::decode::Error), // decode
    #[error("signature: {0}")]
    Signature(#[from] solana_sdk::signature::ParseSignatureError),
    #[error("bincode: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("pubkey: {0}")]
    Pubkey(#[from] solana_sdk::pubkey::ParsePubkeyError),
    #[error("from slice: {0}")]
    Slice(#[from] TryFromSliceError),
    #[error("crypto: {0}")]
    Crypto(#[from] helium_crypto::Error),
    #[error("decode: {0}")]
    Decode(String),
}

impl DecodeError {
    pub fn other<S: ToString>(reason: S) -> Self {
        Self::Decode(reason.to_string())
    }
}
