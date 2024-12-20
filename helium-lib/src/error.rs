use crate::{anchor_client, client, onboarding, solana_client, token};
use std::{array::TryFromSliceError, num::TryFromIntError};
use thiserror::Error;

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
    #[error("Account already exists")]
    AccountExists,
    #[error("Account non existent: {0}")]
    AccountAbsent(String),
    #[error("DAS client: {0}")]
    Das(#[from] client::DasClientError),
    #[error("grpc: {0}")]
    Grpc(#[from] tonic::Status),
    #[error("service: {0}")]
    Service(#[from] helium_proto::services::Error),
    #[error("price client: {0}")]
    Price(#[from] token::price::PriceError),
    #[error("rest client: {0}")]
    Rest(#[from] reqwest::Error),
    #[error("system time: {0}")]
    Time(#[from] std::time::SystemTimeError),
    #[error("program: {0}")]
    Program(#[from] solana_program::program_error::ProgramError),
    #[error("solana: {0}")]
    Solana(Box<solana_client::client_error::ClientError>),
    #[error("solana transaction: {0}")]
    SolanaTransaction(#[from] solana_sdk::transaction::TransactionError),
    #[error("solana pubsub: {0}")]
    SolanaPubsub(#[from] solana_client::pubsub_client::PubsubClientError),
    #[error("tpu sender: {0}")]
    TPUSender(#[from] solana_client::tpu_client::TpuSenderError),
    #[error("signing: {0}")]
    Signing(#[from] solana_sdk::signer::SignerError),
    #[error("crypto: {0}")]
    Crypto(#[from] helium_crypto::Error),
    #[error("decode: {0}")]
    Decode(#[from] DecodeError),
    #[error("encode: {0}")]
    Encode(#[from] EncodeError),
    #[error("Wallet is not configured")]
    WalletUnconfigured,
    #[error("error: {0}")]
    Error(String),
}

impl From<Box<dyn std::error::Error>> for Error {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        Self::other(err.to_string())
    }
}

impl From<solana_client::client_error::ClientError> for Error {
    fn from(value: solana_client::client_error::ClientError) -> Self {
        Self::Solana(Box::new(value))
    }
}

impl Error {
    pub fn account_not_found() -> Self {
        anchor_client::ClientError::AccountNotFound.into()
    }

    pub fn account_exists() -> Self {
        Self::AccountExists.into()
    }

    pub fn is_account_not_found(&self) -> bool {
        use solana_client::{
            client_error::{
                ClientError as SolanaClientError, ClientErrorKind as SolanaClientErrorKind,
            },
            rpc_request::RpcError as SolanaClientRpcError,
        };
        match self {
            Self::Anchor(anchor_client::ClientError::AccountNotFound) => true,
            Self::Solana(client_error) => matches!(client_error.as_ref(), SolanaClientError {
                    kind: SolanaClientErrorKind::RpcError(SolanaClientRpcError::ForUser(msg)),
                    ..
                } if msg.starts_with("AccountNotFound")),

            Self::Das(das_error) => das_error.is_account_not_found(),
            _ => false,
        }
    }

    pub fn other<S: ToString>(reason: S) -> Self {
        Self::Error(reason.to_string())
    }
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
