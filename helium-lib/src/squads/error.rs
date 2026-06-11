//! Squads-specific error types. Grouped into sub-enums per failure
//! domain so call sites and tests can match at whichever level is most
//! useful. Helper constructors on the outer `SquadsError` route to the
//! right sub-enum so most call sites don't need to know the structure.
//!
//! Re-exported as `crate::squads::SquadsError` and friends via the
//! parent module's `pub use self::error::*`.

use crate::error::Error;
use thiserror::Error;

/// Closed set of fields that go through the Squads compactor's u8/u16
/// length casts. Lifting these from `&'static str` to a typed enum
/// gives compile-time enforcement at the ~13 `try_u8`/`try_u16` call
/// sites — a typo (`"acount_keys"`) would silently change the error
/// message under the string-based form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageField {
    NumSigners,
    NumWritableSigners,
    NumWritableNonSigners,
    AccountKeys,
    Instructions,
    InstructionAccounts,
    InstructionDataLen,
    LutCount,
    LutWritableIndexes,
    LutReadonlyIndexes,
    LutResolvedIndex,
    EphemeralSignerIndex,
}

impl std::fmt::Display for MessageField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NumSigners => "num_signers",
            Self::NumWritableSigners => "num_writable_signers",
            Self::NumWritableNonSigners => "num_writable_non_signers",
            Self::AccountKeys => "account_keys",
            Self::Instructions => "instructions",
            Self::InstructionAccounts => "instruction_accounts",
            Self::InstructionDataLen => "instruction_data_len",
            Self::LutCount => "lut_count",
            Self::LutWritableIndexes => "lut_writable_indexes",
            Self::LutReadonlyIndexes => "lut_readonly_indexes",
            Self::LutResolvedIndex => "lut_resolved_index",
            Self::EphemeralSignerIndex => "ephemeral_signer_index",
        })
    }
}

/// Index fields in `MultisigCompiledInstruction` validated against the
/// resolved key list during inspect decode. Sibling to `MessageField`
/// (which tracks wire-format *length* casts); separated to keep each
/// enum's domain tight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompiledInstructionField {
    ProgramIdIndex,
    AccountIndex,
}

impl std::fmt::Display for CompiledInstructionField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ProgramIdIndex => "program_id_index",
            Self::AccountIndex => "account_index",
        })
    }
}

/// Kind of address the user passed when an `--index` is required.
/// Closed set so the error message can't drift via typo and callers
/// can build it from `Version` without a stringly-typed step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    V3Multisig,
    V4Multisig,
    Vault,
}

impl std::fmt::Display for IndexKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::V3Multisig => "v3 Multisig",
            Self::V4Multisig => "v4 Multisig",
            Self::Vault => "vault",
        })
    }
}

/// Squads multisig integration errors. Grouped into sub-enums per
/// failure domain (RPC bypass, vault/multisig resolution, membership
/// pre-flight, wire-format encoding) so call sites and tests can match
/// at whichever level is most useful. Helper constructors route to the
/// right sub-enum so most call sites don't need to know the structure.
#[derive(Debug, Error)]
pub enum SquadsError {
    #[error("rpc: {0}")]
    Rpc(#[from] SquadsRpcError),
    #[error("resolution: {0}")]
    Resolution(#[from] SquadsResolutionError),
    #[error("membership: {0}")]
    Membership(#[from] SquadsMembershipError),
    #[error("encoding: {0}")]
    Encoding(#[from] SquadsEncodingError),
}

/// JSON-RPC failure modes the squads module surfaces from raw
/// `getMultipleAccounts` calls. Distinct from the generic `Solana` /
/// `solana_client::ClientError` shape because we bypass solana-client's
/// decoder (its strict `rent_epoch: u64` deserializer trips on the
/// rent-exempt sentinel).
#[derive(Debug, Error)]
pub enum SquadsRpcError {
    /// JSON-RPC server returned an error response instead of a result.
    /// Surfaces the server's own code + message rather than burying
    /// them as "missing field `result`" deserialize errors.
    #[error("{method} rpc error {code}: {message}")]
    Server {
        method: &'static str,
        code: i64,
        message: String,
    },
    /// JSON-RPC response had neither `result` nor `error` field.
    /// Indicates a non-JSON-RPC-compliant server.
    #[error("{method} response had neither result nor error field")]
    Malformed { method: &'static str },
}

/// User-supplied address couldn't be resolved into the (multisig,
/// transaction index) pair voter / executor / inspector commands need.
#[derive(Debug, Error)]
pub enum SquadsResolutionError {
    /// User passed a multisig or vault PDA without `--index`.
    #[error("{target} is a {kind} — pass --index <n> for the proposal")]
    IndexRequired {
        target: solana_sdk::pubkey::Pubkey,
        kind: IndexKind,
    },
    /// Couldn't find a Squads transaction in the vault's recent
    /// signatures during vault → multisig resolution.
    #[error("could not resolve {vault} to a multisig from the last {scan_limit} signatures")]
    VaultScanFailed {
        vault: solana_sdk::pubkey::Pubkey,
        scan_limit: usize,
    },
    /// A vault PDA resolved to a multisig, but that multisig's owner
    /// isn't a Squads program. Distinct from the simple wrong-owner
    /// case because the resolution went through the cache/scan path.
    #[error("{address} resolved to {multisig} but owner is {got}, not Squads")]
    OwnerMismatch {
        address: solana_sdk::pubkey::Pubkey,
        multisig: solana_sdk::pubkey::Pubkey,
        got: solana_sdk::pubkey::Pubkey,
    },
}

/// Wallet membership / permission failures, surfaced as pre-flight
/// checks before submitting a vote / execute / propose to chain.
#[derive(Debug, Error)]
pub enum SquadsMembershipError {
    /// Wallet's pubkey isn't in the multisig's member list. Surfaced
    /// as a pre-flight check so the user gets a clear local error
    /// rather than the on-chain program rejecting their submission.
    #[error("{wallet} is not a member of multisig {multisig}")]
    NotAMember {
        wallet: solana_sdk::pubkey::Pubkey,
        multisig: solana_sdk::pubkey::Pubkey,
    },
    /// Wallet is a member of the multisig but lacks the permission
    /// the requested action needs (v4-only — v3 members have all
    /// permissions implicitly).
    #[error("{wallet} is a member of {multisig} but lacks {action} permission")]
    MissingPermission {
        wallet: solana_sdk::pubkey::Pubkey,
        multisig: solana_sdk::pubkey::Pubkey,
        action: &'static str,
    },
}

/// Wire-format / on-chain encoding invariants the squads module
/// enforces at compaction or decode time. Each variant captures a
/// constraint that, if silently truncated or skipped, would either
/// produce a corrupt on-chain proposal or render an opaque inspect
/// view.
#[derive(Debug, Error)]
pub enum SquadsEncodingError {
    /// User-supplied transaction index exceeds v3's u32 width.
    #[error("v3 transaction index {index} exceeds u32 range")]
    V3IndexOutOfRange { index: u64 },
    /// On-chain `multisig.transaction_index + 1` would overflow u64.
    /// Effectively never happens (would require ~10^19 proposals on
    /// one multisig) but checked at the proposer side anyway.
    #[error("multisig transaction_index overflow")]
    TransactionIndexOverflow,
    /// v3 execute_transaction's `account_list` u8 indexes overflowed.
    /// 256+ unique accounts across all sub-instructions can't be
    /// addressed by a single u8 index.
    #[error("v3 execute remaining_accounts overflow ({count} > 255)")]
    RemainingAccountsOverflow { count: usize },
    /// Squads' inner-message wire format constrains many counts to u8
    /// (account_keys, instructions, accounts-per-ix, LUTs) or u16
    /// (instruction data length). Exceeding these limits would
    /// silently truncate via `as u8`/`as u16` and produce a corrupted
    /// `transaction_message` on-chain — surface a clean error at
    /// compaction time instead.
    #[error("squads message {field} {value} exceeds wire-format limit {limit}")]
    MessageFieldOverflow {
        field: MessageField,
        value: usize,
        limit: usize,
    },
    /// `MultisigCompiledInstruction.{program_id_index, account_indexes}`
    /// pointed beyond the resolved key list — indicates a corrupt or
    /// malformed on-chain proposal. Renders the proposal opaque rather
    /// than silently falling back to `Pubkey::default()`.
    #[error("compiled instruction {field} {index} out of range (resolved key count {count})")]
    InstructionIndexOutOfRange {
        field: CompiledInstructionField,
        index: usize,
        count: usize,
    },
    /// On-chain proposal status timestamp doesn't fit chrono's
    /// representable range. Should never happen for a real Squads
    /// proposal but flagged rather than silently rendered as 1970.
    #[error("proposal status timestamp {timestamp} out of range")]
    InvalidStatusTimestamp { timestamp: i64 },
    /// LUT writable / readonly index referenced an entry past the end
    /// of the resolved address table. Squads' on-chain handler reads
    /// the same LUT but won't tolerate a missing slot — silently
    /// dropping locally would produce a misaligned account list and
    /// ship a tx the validator rejects (or, worse, mis-applies). Bail
    /// loudly instead.
    #[error("LUT {table} index {index} exceeds table size {size}")]
    LutIndexOutOfRange {
        table: solana_sdk::pubkey::Pubkey,
        index: u8,
        size: usize,
    },
}

/// Lift each sub-enum into the workspace `Error` in one `?` step,
/// alongside the `From<sub> -> SquadsError` derived by thiserror.
macro_rules! sub_into_error {
    ($($ty:ty),* $(,)?) => {
        $(
            impl From<$ty> for Error {
                fn from(value: $ty) -> Self {
                    SquadsError::from(value).into()
                }
            }
        )*
    };
}
sub_into_error!(
    SquadsRpcError,
    SquadsResolutionError,
    SquadsMembershipError,
    SquadsEncodingError,
);

impl SquadsError {
    pub fn rpc(method: &'static str, code: i64, message: impl Into<String>) -> Self {
        SquadsRpcError::Server {
            method,
            code,
            message: message.into(),
        }
        .into()
    }

    pub fn malformed_rpc(method: &'static str) -> Self {
        SquadsRpcError::Malformed { method }.into()
    }

    pub fn index_required(target: solana_sdk::pubkey::Pubkey, kind: IndexKind) -> Self {
        SquadsResolutionError::IndexRequired { target, kind }.into()
    }

    pub fn v3_index_out_of_range(index: u64) -> Self {
        SquadsEncodingError::V3IndexOutOfRange { index }.into()
    }

    pub fn transaction_index_overflow() -> Self {
        SquadsEncodingError::TransactionIndexOverflow.into()
    }

    pub fn vault_resolution_failed(vault: solana_sdk::pubkey::Pubkey, scan_limit: usize) -> Self {
        SquadsResolutionError::VaultScanFailed { vault, scan_limit }.into()
    }

    pub fn remaining_accounts_overflow(count: usize) -> Self {
        SquadsEncodingError::RemainingAccountsOverflow { count }.into()
    }

    pub fn resolved_owner_mismatch(
        address: solana_sdk::pubkey::Pubkey,
        multisig: solana_sdk::pubkey::Pubkey,
        got: solana_sdk::pubkey::Pubkey,
    ) -> Self {
        SquadsResolutionError::OwnerMismatch {
            address,
            multisig,
            got,
        }
        .into()
    }

    pub fn message_field_overflow(field: MessageField, value: usize, limit: usize) -> Self {
        SquadsEncodingError::MessageFieldOverflow {
            field,
            value,
            limit,
        }
        .into()
    }

    /// `u8::try_from` shorthand: route the overflow into a
    /// `MessageFieldOverflow` with the correct limit (255).
    pub fn try_u8(value: usize, field: MessageField) -> Result<u8, Self> {
        u8::try_from(value)
            .map_err(|_| Self::message_field_overflow(field, value, u8::MAX as usize))
    }

    /// `u16::try_from` shorthand for instruction data lengths.
    pub fn try_u16(value: usize, field: MessageField) -> Result<u16, Self> {
        u16::try_from(value)
            .map_err(|_| Self::message_field_overflow(field, value, u16::MAX as usize))
    }

    pub fn instruction_index_out_of_range(
        field: CompiledInstructionField,
        index: usize,
        count: usize,
    ) -> Self {
        SquadsEncodingError::InstructionIndexOutOfRange {
            field,
            index,
            count,
        }
        .into()
    }

    pub fn invalid_status_timestamp(timestamp: i64) -> Self {
        SquadsEncodingError::InvalidStatusTimestamp { timestamp }.into()
    }

    pub fn lut_index_out_of_range(
        table: solana_sdk::pubkey::Pubkey,
        index: u8,
        size: usize,
    ) -> Self {
        SquadsEncodingError::LutIndexOutOfRange { table, index, size }.into()
    }

    pub fn not_a_member(
        wallet: solana_sdk::pubkey::Pubkey,
        multisig: solana_sdk::pubkey::Pubkey,
    ) -> Self {
        SquadsMembershipError::NotAMember { wallet, multisig }.into()
    }

    pub fn missing_permission(
        wallet: solana_sdk::pubkey::Pubkey,
        multisig: solana_sdk::pubkey::Pubkey,
        action: &'static str,
    ) -> Self {
        SquadsMembershipError::MissingPermission {
            wallet,
            multisig,
            action,
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `try_u8` returns the cast on success and a structured
    /// `MessageFieldOverflow` on overflow. Pins the field/value/limit
    /// shape so a regression at any of the ~13 compactor call sites
    /// surfaces here.
    #[test]
    fn try_u8_overflow() {
        assert_eq!(
            SquadsError::try_u8(255, MessageField::AccountKeys).unwrap(),
            255u8,
        );
        let err = SquadsError::try_u8(256, MessageField::AccountKeys).unwrap_err();
        match err {
            SquadsError::Encoding(SquadsEncodingError::MessageFieldOverflow {
                field,
                value,
                limit,
            }) => {
                assert_eq!(field, MessageField::AccountKeys);
                assert_eq!(value, 256);
                assert_eq!(limit, u8::MAX as usize);
            }
            other => panic!("expected MessageFieldOverflow, got {other:?}"),
        }
    }

    /// `try_u16` mirrors the u8 helper for instruction data lengths.
    #[test]
    fn try_u16_overflow() {
        assert_eq!(
            SquadsError::try_u16(65_535, MessageField::InstructionDataLen).unwrap(),
            u16::MAX,
        );
        let err = SquadsError::try_u16(65_536, MessageField::InstructionDataLen).unwrap_err();
        match err {
            SquadsError::Encoding(SquadsEncodingError::MessageFieldOverflow {
                field,
                value,
                limit,
            }) => {
                assert_eq!(field, MessageField::InstructionDataLen);
                assert_eq!(value, 65_536);
                assert_eq!(limit, u16::MAX as usize);
            }
            other => panic!("expected MessageFieldOverflow, got {other:?}"),
        }
    }

    /// `MessageField` Display output is part of error-message contract;
    /// pin a couple of variants so accidental rename surfaces here.
    #[test]
    fn message_field_display_labels() {
        assert_eq!(format!("{}", MessageField::AccountKeys), "account_keys");
        assert_eq!(
            format!("{}", MessageField::InstructionDataLen),
            "instruction_data_len",
        );
        assert_eq!(
            format!("{}", MessageField::LutResolvedIndex),
            "lut_resolved_index",
        );
    }
}
