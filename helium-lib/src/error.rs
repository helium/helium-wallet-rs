use crate::{
    anchor_client, anchor_lang, client, hotspot::cert, jupiter, onboarding, solana_client, squads,
    token,
};
use solana_sdk::signature::Signature;
use std::{array::TryFromSliceError, num::TryFromIntError, time::Duration};
use thiserror::Error;

/// Main error type for helium-lib operations.
#[derive(Error, Debug)]
pub enum Error {
    #[cfg(feature = "mnemonic")]
    #[error("mnemonic: {0}")]
    Mnemonic(#[from] helium_mnemonic::MnmemonicError),
    #[error("onboarding: {0}")]
    Onboarding(#[from] onboarding::OnboardingError),
    #[error("anchor client: {0}")]
    Anchor(Box<anchor_client::ClientError>),
    #[error("anchor lang: {0}")]
    AnchorLang(#[from] anchor_lang::error::Error),
    #[error("DAS client: {0}")]
    Das(#[from] client::DasClientError),
    #[error("cert client: {0}")]
    Cert(#[from] cert::ClientError),
    #[error("grpc: {0}")]
    Grpc(Box<tonic::Status>),
    #[error("transport: {0}")]
    Transport(#[from] tonic::transport::Error),
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
    #[error("instruction: {0}")]
    Instruction(#[from] solana_sdk::instruction::InstructionError),
    /// Transaction building/packing errors from solana-transaction-utils
    #[error("transaction: {0}")]
    Transaction(#[from] solana_transaction_utils::error::Error),
    /// Transaction confirmation errors (polling for finalization)
    #[error("confirmation: {0}")]
    Confirmation(#[from] ConfirmationError),
    #[error("message: {0}")]
    Compile(#[from] solana_sdk::message::CompileError),
    #[error("signing: {0}")]
    Signing(#[from] solana_sdk::signer::SignerError),
    #[error("crypto: {0}")]
    Crypto(#[from] helium_crypto::Error),
    #[error("decode: {0}")]
    Decode(#[from] DecodeError),
    #[error("encode: {0}")]
    Encode(#[from] EncodeError),
    #[error("tuktuk: {0}")]
    Tuktuk(#[from] tuktuk_sdk::error::Error),
    #[error("jupiter: {0}")]
    Jupiter(#[from] jupiter::JupiterError),
    #[error("squads: {0}")]
    Squads(#[from] squads::SquadsError),
    /// Asset owner doesn't match the caller's expectation. Surfaced by
    /// owner pre-checks (e.g. the Squads-mode asset/hotspot wrappers,
    /// which enforce this so a proposer can't ship an un-executable
    /// proposal whose `leaf_owner` the vault can't satisfy at execute
    /// time) with both addresses so the user can compare.
    #[error("asset {asset} is owned by {actual}, expected {expected}")]
    WrongAssetOwner {
        asset: solana_sdk::pubkey::Pubkey,
        actual: solana_sdk::pubkey::Pubkey,
        expected: solana_sdk::pubkey::Pubkey,
    },
}

impl From<solana_client::client_error::ClientError> for Error {
    fn from(value: solana_client::client_error::ClientError) -> Self {
        Self::Solana(Box::new(value))
    }
}

impl From<anchor_client::ClientError> for Error {
    fn from(value: anchor_client::ClientError) -> Self {
        Self::Anchor(Box::new(value))
    }
}

impl From<tonic::Status> for Error {
    fn from(value: tonic::Status) -> Self {
        Self::Grpc(Box::new(value))
    }
}

impl Error {
    /// Create an error indicating a Solana account was not found.
    pub fn account_not_found() -> Self {
        anchor_client::ClientError::AccountNotFound.into()
    }

    /// Check whether this error represents a missing Solana account.
    pub fn is_account_not_found(&self) -> bool {
        use solana_client::{
            client_error::{
                ClientError as SolanaClientError, ClientErrorKind as SolanaClientErrorKind,
            },
            rpc_request::RpcError as SolanaClientRpcError,
        };
        match self {
            Self::Anchor(client_error) => matches!(
                client_error.as_ref(),
                anchor_client::ClientError::AccountNotFound
            ),
            Self::Solana(client_error) => matches!(client_error.as_ref(), SolanaClientError {
                    kind: SolanaClientErrorKind::RpcError(SolanaClientRpcError::ForUser(msg)),
                    ..
                } if msg.starts_with("AccountNotFound")),

            Self::Das(das_error) => das_error.is_account_not_found(),
            _ => false,
        }
    }
}

/// Errors during data encoding (protobuf, JSON, bincode, etc.).
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("proto: {0}")]
    Proto(#[from] helium_proto::EncodeError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("bincode: {0}")]
    Bincode(#[from] bincode::Error),
    #[error("h3: {0}")]
    H3(#[from] h3o::error::InvalidLatLng),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("encode: {0}")]
    Encode(String),
}

impl EncodeError {
    pub fn other<S: ToString>(reason: S) -> Self {
        Self::Encode(reason.to_string())
    }

    /// Wraps a Borsh / Anchor serialize failure with the type's name
    /// for context. Used at instruction-data construction sites where
    /// the borsh error alone wouldn't tell the user which arg struct
    /// failed.
    pub fn borsh<E: std::fmt::Display>(type_name: &str, source: E) -> Self {
        Self::Encode(format!("{type_name}: {source}"))
    }
}

/// Errors during data decoding (protobuf, base58, hex, etc.).
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
    #[error("prost: {0}")]
    Enum(#[from] helium_proto::UnknownEnumValue), // decode
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

    /// Account isn't owned by the expected program.
    pub fn wrong_owner(
        address: &solana_sdk::pubkey::Pubkey,
        expected: &str,
        got: &solana_sdk::pubkey::Pubkey,
    ) -> Self {
        Self::Decode(format!("{address} owner is {got}, expected {expected}"))
    }

    /// Account doesn't match the 8-byte Anchor discriminator for the
    /// expected type.
    pub fn wrong_discriminator(address: &solana_sdk::pubkey::Pubkey, account_type: &str) -> Self {
        Self::Decode(format!(
            "{address} is not a {account_type} account (bad discriminator)"
        ))
    }

    /// Borsh / Anchor deserialize failure for a specific account.
    pub fn deserialize<E: std::fmt::Display>(
        address: &solana_sdk::pubkey::Pubkey,
        account_type: &str,
        source: E,
    ) -> Self {
        Self::Decode(format!("{account_type} {address}: {source}"))
    }
}

/// Errors related to transaction confirmation polling
#[derive(Debug, Error)]
pub enum ConfirmationError {
    /// Transaction signature not found on-chain (may have been dropped or never sent)
    #[error("signature {signature} not found: {reason}")]
    NotFound {
        signature: Signature,
        reason: String,
    },

    /// Transaction failed on-chain with a program error
    #[error("transaction {signature} failed: {error}")]
    Failed { signature: Signature, error: String },

    /// Confirmation polling timed out before reaching finalized status
    #[error("timeout after {duration:?} waiting for {count} signatures")]
    Timeout { duration: Duration, count: usize },

    /// Multiple signatures failed to confirm
    #[error(
        "batch confirmation failed: {succeeded} succeeded, {failed} failed, {not_found} not found"
    )]
    BatchFailed {
        succeeded: usize,
        failed: usize,
        not_found: usize,
    },
}

impl ConfirmationError {
    /// Create a NotFound error for a signature that wasn't found on-chain
    pub fn not_found(signature: Signature, reason: impl Into<String>) -> Self {
        Self::NotFound {
            signature,
            reason: reason.into(),
        }
    }

    /// Create a Failed error for a transaction that failed on-chain
    pub fn failed(signature: Signature, error: impl Into<String>) -> Self {
        Self::Failed {
            signature,
            error: error.into(),
        }
    }

    /// Create a Timeout error when confirmation polling exceeded the deadline
    pub fn timeout(duration: Duration, count: usize) -> Self {
        Self::Timeout { duration, count }
    }

    /// Create a BatchFailed error summarizing batch confirmation results
    pub fn batch_failed(succeeded: usize, failed: usize, not_found: usize) -> Self {
        Self::BatchFailed {
            succeeded,
            failed,
            not_found,
        }
    }
}

impl Error {
    /// Helper to create a confirmation not found error
    pub fn confirmation_not_found(signature: Signature, reason: impl Into<String>) -> Self {
        ConfirmationError::not_found(signature, reason).into()
    }

    /// Helper to create a confirmation failed error
    pub fn confirmation_failed(signature: Signature, error: impl Into<String>) -> Self {
        ConfirmationError::failed(signature, error).into()
    }

    /// Helper to create a confirmation timeout error
    pub fn confirmation_timeout(duration: Duration, count: usize) -> Self {
        ConfirmationError::timeout(duration, count).into()
    }
}
