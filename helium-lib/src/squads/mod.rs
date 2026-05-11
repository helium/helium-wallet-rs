//! Squads multisig integration.
//!
//! v4 types come from the upstream `squads-multisig-program` crate
//! (`Squads-Protocol/v4` main). The crates.io release pins anchor 0.29 / solana
//! 1.x and is incompatible; main branch is on anchor 0.32 / solana 2.2.
//!
//! v3 types come from the on-chain IDL via `declare_program!(squads_mpl)`. The
//! `squads-mpl` crate is archived on a Solana 1.x toolchain incompatible with
//! this workspace; the IDL path works because v3 doesn't use parameterized
//! types (unlike v4's `SmallVec<L,T>`).

mod cache;
pub mod error;
pub mod v3;
pub mod v4;

pub use self::error::{
    CompiledInstructionField, IndexKind, MessageField, SquadsEncodingError, SquadsError,
    SquadsMembershipError, SquadsResolutionError, SquadsRpcError,
};

use crate::{
    client::SolanaRpcClient, error::DecodeError, error::Error, keypair::Pubkey,
    programs::KnownProgram,
};

/// Multisig PDA — the Squads-program-owned account that holds member
/// list, threshold, and `transaction_index`. Distinct newtype from
/// `VaultKey` so a vault can't be silently passed where a multisig is
/// expected (or vice versa) — both are `Pubkey` under the hood and the
/// confusion is the kind of bug that grows roots before anyone notices.
///
/// `from_pubkey` is non-validating; the trusted source of `MultisigKey`
/// values is `resolve_to_multisig`, which checks the on-chain owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultisigKey(Pubkey);

/// Vault PDA — the system-program-owned authority that holds funds and
/// signs CPI calls on behalf of the multisig. Distinct newtype from
/// `MultisigKey` (see that type's doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VaultKey(Pubkey);

macro_rules! impl_pubkey_newtype {
    ($ty:ident) => {
        impl $ty {
            /// Wrap a pubkey, asserting it's the corresponding kind.
            /// Non-validating; trusted internal sources only.
            pub fn from_pubkey(pubkey: Pubkey) -> Self {
                Self(pubkey)
            }
            /// Borrow the wrapped pubkey for APIs that take `&Pubkey`.
            pub fn as_pubkey(&self) -> &Pubkey {
                &self.0
            }
            /// Unwrap into the bare pubkey. Use when crossing into
            /// helium-lib instruction builders that work with Pubkey.
            pub fn into_pubkey(self) -> Pubkey {
                self.0
            }
        }
        impl AsRef<Pubkey> for $ty {
            fn as_ref(&self) -> &Pubkey {
                &self.0
            }
        }
        impl AsRef<[u8]> for $ty {
            fn as_ref(&self) -> &[u8] {
                self.0.as_ref()
            }
        }
        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                crate::keypair::serde_pubkey::serialize(&self.0, s)
            }
        }
    };
}

impl_pubkey_newtype!(MultisigKey);
impl_pubkey_newtype!(VaultKey);

/// Either-version structured view of a proposal returned by the top-level
/// `get_proposal_info`. The internally-tagged `version` field makes the
/// V3/V4 distinction explicit in the JSON output rather than relying on
/// field-set differences between the two arms (which a future shape
/// change could quietly invalidate). Downstream consumers can pivot on
/// `version: "v3" | "v4"`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "version", rename_all = "lowercase")]
pub enum ProposalView {
    V3(v3::ProposalInfo),
    V4(v4::ProposalInfo),
}

/// At-a-glance signals a reviewer needs to triage a proposal: did it hit
/// threshold, has the multisig config changed since this proposal was
/// created (which silently invalidates it), what programs does it touch,
/// and is anything unrecognized. v3 always reports `stale: false` — v3
/// doesn't record the multisig change index on its transaction accounts
/// so we can't detect staleness from the transaction alone.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProposalSummary {
    /// `"3/3"` etc. — votes-approved / threshold required.
    pub approvals: String,
    /// True if v4 `transaction_index <= multisig.stale_transaction_index`.
    /// Always false on v3 (see struct doc).
    pub stale: bool,
    /// Deduped, in-order list of programs the proposal will invoke. Maps
    /// to friendly names for known programs; unknown program IDs go to
    /// `unknown_programs` instead.
    pub programs: Vec<KnownProgram>,
    /// Program IDs not in the recognized set. Any non-empty value here is
    /// a red flag worth investigating before approving.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_programs: Vec<String>,
    /// Count of instructions where the program ships an IDL but the
    /// instruction's discriminator wasn't found in it. Either the shipped
    /// IDL is stale (re-run `gen_idl.sh`) or the proposal calls an
    /// instruction that's been removed from the program. Either way,
    /// non-zero is a flag to investigate before approving.
    #[serde(skip_serializing_if = "is_zero")]
    pub unknown_methods: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Build a `ProposalSummary` from a fully-decoded instruction list and the
/// version-specific approvals/stale signals. Used by both v3 and v4 paths
/// so the summary shape stays consistent.
pub(crate) fn build_summary(
    approvals: String,
    stale: bool,
    instructions: &[InstructionInfo],
) -> ProposalSummary {
    let mut programs: Vec<KnownProgram> = Vec::new();
    let mut unknown_programs: Vec<String> = Vec::new();
    let mut unknown_methods: u32 = 0;
    for ix in instructions {
        match ix.program {
            Some(kp) => {
                if !programs.contains(&kp) {
                    programs.push(kp);
                }
                if ix.method.is_none() && kp.has_idl() {
                    unknown_methods += 1;
                }
            }
            None => {
                let id = ix.program_id.to_string();
                if !unknown_programs.contains(&id) {
                    unknown_programs.push(id);
                }
            }
        }
    }
    ProposalSummary {
        approvals,
        stale,
        programs,
        unknown_programs,
        unknown_methods,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProposalVotes {
    pub approved: usize,
    pub rejected: usize,
    pub cancelled: usize,
}

/// One row in the output of `list_open_proposals` — minimal context to
/// triage and feed back into `inspect`/`approve`/etc. Inner instructions
/// aren't decoded here; reach for `inspect <transaction>` when you need
/// to verify what a proposal will do.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProposalListEntry {
    pub index: u64,
    /// Transaction PDA — pass to `inspect`/`approve`/`reject`/`cancel`/
    /// `execute` to act on the proposal without re-deriving from index.
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub transaction: Pubkey,
    /// Lowercase status: `draft`, `active`, `approved` (v4),
    /// `execute_ready` (v3), `executing` (v4 transient).
    pub status: &'static str,
    /// ISO8601 timestamp of the current status (v4 only — v3
    /// `MsTransaction` doesn't carry per-status timestamps). Lets a
    /// reviewer eyeball the age of a still-pending proposal without
    /// running `inspect`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    pub votes: ProposalVotes,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstructionInfo {
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub program_id: Pubkey,
    /// Recognized program identity, when known. `None` falls back to
    /// displaying the raw `program_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<KnownProgram>,
    /// Snake-cased method name resolved from the program's IDL when both
    /// the program is recognized and the discriminator matches a known
    /// instruction. The raw `discriminator` stays alongside as a binary
    /// identity check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'static str>,
    /// Decoded args as structured JSON, populated when the program ships
    /// an IDL we can decode AND the body bytes round-trip successfully
    /// against the instruction's args definition. Field names and shapes
    /// come straight from the IDL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    pub accounts: Vec<InstructionAccountRef>,
    pub data_len: usize,
    /// Anchor instruction discriminator (first 8 data bytes), base58-encoded.
    /// Useful for matching against known program IDLs.
    pub discriminator: Option<String>,
    pub data_b58: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstructionAccountRef {
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub pubkey: Pubkey,
    pub writable: bool,
    pub signer: bool,
}
use futures::stream::{self, StreamExt};
use serde::Serialize;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_sdk::{commitment_config::CommitmentConfig, signature::Signature};
use solana_transaction_status::{
    option_serializer::OptionSerializer, UiLoadedAddresses, UiTransactionEncoding,
};
use std::{collections::HashSet, str::FromStr};

/// Squads program version controlling a multisig.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Version {
    V3,
    V4,
}

impl From<Version> for IndexKind {
    fn from(v: Version) -> Self {
        match v {
            Version::V3 => Self::V3Multisig,
            Version::V4 => Self::V4Multisig,
        }
    }
}

/// First 8 bytes of an account's data as an Anchor discriminator
/// array. `None` if the slice is shorter than 8 bytes. Used by every
/// Squads decode path that branches on the discriminator.
pub(crate) fn read_discriminator(data: &[u8]) -> Option<[u8; 8]> {
    data.get(..8)?.try_into().ok()
}

/// Unified multisig summary across v3 and v4.
#[derive(Debug, Clone, Serialize)]
pub struct MultisigInfo {
    pub address: MultisigKey,
    pub version: Version,
    pub threshold: u16,
    /// v4: last transaction index. v3: same field, widened to u64.
    pub transaction_index: u64,
    pub members: Vec<MemberInfo>,
    /// Set when `address` was originally passed in as a vault PDA and we
    /// resolved it to its parent multisig.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resolved_from_vault: Option<VaultKey>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberInfo {
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub key: Pubkey,
    pub permissions: MemberPermissions,
}

/// Permissions a member has on a multisig. v3 has no per-member permissions —
/// all listed keys can do everything — so v3 members report all three as true.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct MemberPermissions {
    pub propose: bool,
    pub vote: bool,
    pub execute: bool,
}

impl MemberPermissions {
    pub const ALL: Self = Self {
        propose: true,
        vote: true,
        execute: true,
    };

    /// Encode as Squads' on-chain `Permissions::mask` byte. Upstream
    /// `Permission` discriminants ARE the bit values
    /// (`Initiate = 1<<0`, `Vote = 1<<1`, `Execute = 1<<2`), so we
    /// cast and OR directly — no allocations, no duplicated constants.
    pub fn to_mask(self) -> u8 {
        use squads_multisig_program::state::Permission;
        let mut mask = 0u8;
        if self.propose {
            mask |= Permission::Initiate as u8;
        }
        if self.vote {
            mask |= Permission::Vote as u8;
        }
        if self.execute {
            mask |= Permission::Execute as u8;
        }
        mask
    }

    /// Decode from Squads' on-chain `Permissions::mask` byte. Bits
    /// outside the recognized set are ignored — surfacing them would
    /// force every caller to handle a hypothetical.
    pub fn from_mask(mask: u8) -> Self {
        use squads_multisig_program::state::Permission;
        let bit = |p: Permission| mask & (p as u8) != 0;
        Self {
            propose: bit(Permission::Initiate),
            vote: bit(Permission::Vote),
            execute: bit(Permission::Execute),
        }
    }
}

/// Action the wallet is about to take on a multisig. Used to map a
/// CLI command (`squads approve`, `transfer --squads ...`, etc.) to
/// the per-member permission bit Squads' on-chain program will
/// validate, so we can pre-flight the check locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberAction {
    /// Cast a vote on a proposal — `proposal_approve`,
    /// `proposal_reject`, `proposal_cancel` (v4) or the v3 equivalents.
    Vote,
    /// Execute an approved proposal — `vault_transaction_execute`
    /// (v4) or `execute_transaction` (v3).
    Execute,
    /// Create a new proposal — `vault_transaction_create` +
    /// `proposal_create` (v4 only). v3's `create_transaction` is
    /// covered by the same Initiate permission.
    Initiate,
}

impl MemberAction {
    /// Stable label for error messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Vote => "vote",
            Self::Execute => "execute",
            Self::Initiate => "initiate",
        }
    }

    /// True when `perms` grant this action. v3 members report
    /// permissions as all-true so v3 calls always pass.
    pub fn is_granted(self, perms: &MemberPermissions) -> bool {
        match self {
            Self::Vote => perms.vote,
            Self::Execute => perms.execute,
            Self::Initiate => perms.propose,
        }
    }
}

/// Pure membership + permission check against a fetched
/// `MultisigInfo`. Split out from `check_member_permission` so the
/// dispatch can be unit-tested without RPC.
pub fn validate_membership(
    info: &MultisigInfo,
    wallet: &Pubkey,
    action: MemberAction,
) -> Result<(), Error> {
    let entry = info.members.iter().find(|m| m.key == *wallet);
    let multisig_pubkey = info.address.into_pubkey();
    match entry {
        None => Err(SquadsError::not_a_member(*wallet, multisig_pubkey).into()),
        Some(m) if !action.is_granted(&m.permissions) => {
            Err(SquadsError::missing_permission(*wallet, multisig_pubkey, action.label()).into())
        }
        Some(_) => Ok(()),
    }
}

/// Pre-flight check: verify the wallet's pubkey is a member of the
/// multisig and holds the permission `action` requires. Catches "wrong
/// wallet" and "missing permission" before the user pays simulation /
/// compute fees on a doomed submission. Costs one extra RPC call
/// (the multisig account); call sites already have the multisig
/// pubkey from `resolve_proposal_target` or `squads_vault`.
pub async fn check_member_permission<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: &MultisigKey,
    wallet: &Pubkey,
    action: MemberAction,
) -> Result<(), Error> {
    let info = get_multisig_info(client, multisig.as_pubkey()).await?;
    validate_membership(&info, wallet, action)
}

/// Fetch a multisig and return a unified summary. If `address` is a v3 or v4
/// Multisig PDA, it's decoded directly. If it's a system-owned PDA (typically
/// a Squads vault, which is what users normally see in URLs and explorers),
/// we look it up in the local vault cache and fall back to scanning recent
/// transactions; either way we recurse on the resolved multisig.
pub async fn get_multisig_info<C: AsRef<SolanaRpcClient>>(
    client: &C,
    address: &Pubkey,
) -> Result<MultisigInfo, Error> {
    // `resolve` returns both the resolved multisig and the original
    // vault (if the input was a vault PDA), so we don't pay an extra
    // `get_account(address)` to figure out what the input was.
    let Resolution {
        multisig,
        vault: resolved_from,
    } = resolve(client, address).await?;
    let account = client.as_ref().get_account(multisig.as_pubkey()).await?;
    match account.owner {
        v4::PROGRAM_ID => v4::decode_multisig(multisig, &account.data, resolved_from),
        v3::PROGRAM_ID => v3::decode_multisig(multisig, &account.data, resolved_from),
        other => Err(
            SquadsError::resolved_owner_mismatch(*address, multisig.into_pubkey(), other).into(),
        ),
    }
}

/// (Version, multisig, transaction_index) triple used by the voter-side
/// CLI commands. Built by `resolve_proposal_target` from the same
/// targets that `inspect_target` accepts. `multisig` is typed as
/// `MultisigKey` so downstream ix builders can't be passed a vault PDA
/// by accident.
#[derive(Debug, Clone, Copy)]
pub struct ProposalTarget {
    pub version: Version,
    pub multisig: MultisigKey,
    pub index: u64,
}

/// Identification of a Squads-relevant address by its owner and (for
/// Squads-owned accounts) the discriminator at the head of its body.
/// Drives the dispatch shared by `resolve_proposal_target` and
/// `inspect_target`.
#[derive(Debug)]
enum TargetKind {
    /// A v3 or v4 transaction-bearing account that self-identifies its
    /// multisig and index — Proposal / VaultTransaction /
    /// ConfigTransaction / Batch on v4, MsTransaction on v3.
    Transaction {
        version: Version,
        multisig: MultisigKey,
        index: u64,
    },
    /// A Squads multisig PDA — caller must supply the index out of band.
    Multisig { version: Version },
    /// A vault / authority PDA (system-owned) — caller must supply
    /// both the index and resolve the multisig separately.
    Vault,
}

fn classify_target(target: &Pubkey, owner: &Pubkey, data: &[u8]) -> Result<TargetKind, Error> {
    if *owner == v4::PROGRAM_ID {
        Ok(match v4::extract_target(target, data)? {
            Some((multisig, index)) => TargetKind::Transaction {
                version: Version::V4,
                multisig: MultisigKey::from_pubkey(multisig),
                index,
            },
            None => TargetKind::Multisig {
                version: Version::V4,
            },
        })
    } else if *owner == v3::PROGRAM_ID {
        Ok(match v3::extract_target(target, data)? {
            Some((multisig, index)) => TargetKind::Transaction {
                version: Version::V3,
                multisig: MultisigKey::from_pubkey(multisig),
                index,
            },
            None => TargetKind::Multisig {
                version: Version::V3,
            },
        })
    } else if *owner == solana_sdk::system_program::ID {
        Ok(TargetKind::Vault)
    } else {
        Err(DecodeError::wrong_owner(target, "Squads", owner).into())
    }
}

fn index_required(target: &Pubkey, kind: IndexKind) -> Error {
    SquadsError::index_required(*target, kind).into()
}

/// Resolve any user-provided pointer to a proposal — multisig+index,
/// vault+index, or a self-identifying transaction/proposal PDA — into
/// the (version, multisig, index) triple needed by voter-side
/// instruction builders.
pub async fn resolve_proposal_target<C: AsRef<SolanaRpcClient>>(
    client: &C,
    target: &Pubkey,
    index: Option<u64>,
) -> Result<ProposalTarget, Error> {
    let account = client.as_ref().get_account(target).await?;
    match classify_target(target, &account.owner, &account.data)? {
        TargetKind::Transaction {
            version,
            multisig,
            index,
        } => Ok(ProposalTarget {
            version,
            multisig,
            index,
        }),
        TargetKind::Multisig { version } => {
            let index = index.ok_or_else(|| index_required(target, IndexKind::from(version)))?;
            Ok(ProposalTarget {
                version,
                multisig: MultisigKey::from_pubkey(*target),
                index,
            })
        }
        TargetKind::Vault => {
            let index = index.ok_or_else(|| index_required(target, IndexKind::Vault))?;
            let multisig = resolve_to_multisig(client, target).await?;
            let ms_account = client.as_ref().get_account(multisig.as_pubkey()).await?;
            let version = match ms_account.owner {
                o if o == v4::PROGRAM_ID => Version::V4,
                o if o == v3::PROGRAM_ID => Version::V3,
                other => {
                    return Err(
                        DecodeError::wrong_owner(multisig.as_pubkey(), "Squads", &other).into(),
                    )
                }
            };
            Ok(ProposalTarget {
                version,
                multisig,
                index,
            })
        }
    }
}

/// Fetch and decode a proposal given any one of:
/// - a multisig PDA + explicit index (v3 or v4),
/// - a vault PDA + explicit index (resolved through cache/scan to multisig),
/// - a transaction PDA on its own (v4 VaultTransaction, ConfigTransaction,
///   Batch, or Proposal; v3 MsTransaction) — the multisig and index are
///   read from the account body.
pub async fn inspect_target<C: AsRef<SolanaRpcClient>>(
    client: &C,
    target: &Pubkey,
    index: Option<u64>,
) -> Result<ProposalView, Error> {
    if let Some(index) = index {
        return get_proposal_info(client, target, index).await;
    }
    let account = client.as_ref().get_account(target).await?;
    let (multisig, idx) = match classify_target(target, &account.owner, &account.data)? {
        TargetKind::Transaction {
            multisig, index, ..
        } => (multisig, index),
        TargetKind::Multisig { version } => {
            return Err(index_required(target, IndexKind::from(version)));
        }
        TargetKind::Vault => return Err(index_required(target, IndexKind::Vault)),
    };
    get_proposal_info(client, multisig.as_pubkey(), idx).await
}

/// Fetch and decode a proposal at the given transaction index, dispatching
/// to v3 or v4 based on the multisig's program owner. Vault addresses
/// resolve through the same cache + scan path as `get_multisig_info`.
///
/// `transaction_index` is u64 to match what users see in Squads UIs;
/// values outside u32 range are rejected for v3 multisigs (where the
/// underlying field is u32).
pub async fn get_proposal_info<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig_or_vault: &Pubkey,
    transaction_index: u64,
) -> Result<ProposalView, Error> {
    let multisig = resolve_to_multisig(client, multisig_or_vault).await?;
    let account = client.as_ref().get_account(multisig.as_pubkey()).await?;
    match account.owner {
        v4::PROGRAM_ID => v4::get_proposal_info(client, multisig.as_pubkey(), transaction_index)
            .await
            .map(ProposalView::V4),
        v3::PROGRAM_ID => {
            let index_u32 = u32::try_from(transaction_index)
                .map_err(|_| SquadsError::v3_index_out_of_range(transaction_index))?;
            v3::get_proposal_info(client, multisig.as_pubkey(), index_u32)
                .await
                .map(ProposalView::V3)
        }
        owner => Err(DecodeError::wrong_owner(multisig.as_pubkey(), "Squads", &owner).into()),
    }
}

/// List all open proposals on a multisig — anything still subject to
/// member action: voting (Draft/Active) or execution (v4 Approved, v3
/// ExecuteReady). Finalized states (Executed, Rejected, Cancelled) are
/// not returned. v4 stale proposals (index ≤ `stale_transaction_index`)
/// are skipped — once the multisig config changes, those can no longer
/// be voted on or executed.
pub async fn list_open_proposals<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig_or_vault: &Pubkey,
) -> Result<Vec<ProposalListEntry>, Error> {
    let multisig = resolve_to_multisig(client, multisig_or_vault).await?;
    let account = client.as_ref().get_account(multisig.as_pubkey()).await?;
    match account.owner {
        v4::PROGRAM_ID => v4::list_open_proposals(client, &multisig).await,
        v3::PROGRAM_ID => v3::list_open_proposals(client, &multisig).await,
        owner => Err(DecodeError::wrong_owner(multisig.as_pubkey(), "Squads", &owner).into()),
    }
}

/// Result of resolving any Squads-related address to its multisig.
/// `vault` is set only when `address` was itself a vault PDA — for
/// multisig or transaction-bearing inputs, only the multisig matters
/// and `vault` is `None`.
pub struct Resolution {
    pub multisig: MultisigKey,
    pub vault: Option<VaultKey>,
}

/// Resolve a multisig pubkey from any Squads-related address: a
/// multisig PDA, a vault PDA, or a transaction-bearing account (v4
/// Proposal / VaultTransaction / ConfigTransaction / Batch, v3
/// MsTransaction). Multisig PDAs return themselves; vault PDAs go
/// through the local cache + fallback recent-transaction scan;
/// transaction-bearing accounts have the multisig in their body and
/// are decoded directly via `extract_target`.
pub async fn resolve_to_multisig<C: AsRef<SolanaRpcClient>>(
    client: &C,
    address: &Pubkey,
) -> Result<MultisigKey, Error> {
    Ok(resolve(client, address).await?.multisig)
}

/// Sibling of `resolve_to_multisig` that also surfaces the original
/// vault PDA when the input was a vault. Callers that need both
/// (e.g. `get_multisig_info`) avoid a second `get_account(address)`
/// to reclassify the input.
pub async fn resolve<C: AsRef<SolanaRpcClient>>(
    client: &C,
    address: &Pubkey,
) -> Result<Resolution, Error> {
    let account = client.as_ref().get_account(address).await?;
    let multisig_only = |multisig: Pubkey| Resolution {
        multisig: MultisigKey::from_pubkey(multisig),
        vault: None,
    };
    match account.owner {
        v4::PROGRAM_ID => match v4::extract_target(address, &account.data)? {
            Some((multisig, _index)) => Ok(multisig_only(multisig)),
            None => Ok(multisig_only(*address)),
        },
        v3::PROGRAM_ID => match v3::extract_target(address, &account.data)? {
            Some((multisig, _index)) => Ok(multisig_only(multisig)),
            None => Ok(multisig_only(*address)),
        },
        owner if owner == solana_sdk::system_program::ID => {
            let multisig = match cache::lookup(address) {
                Some(cached) => cached,
                None => {
                    let resolved = resolve_vault_to_multisig(client, address).await?;
                    cache::store(address, &resolved);
                    resolved
                }
            };
            Ok(Resolution {
                multisig: MultisigKey::from_pubkey(multisig),
                vault: Some(VaultKey::from_pubkey(*address)),
            })
        }
        other => Err(DecodeError::wrong_owner(address, "Squads multisig or vault", &other).into()),
    }
}

/// Maximum number of recent signatures to scan when resolving a vault. Most
/// active vaults have a Squads transaction execution within the latest few
/// hundred signatures; idle vaults that only receive deposits may need more,
/// but this cap keeps the resolution bounded.
const VAULT_RESOLUTION_SCAN_LIMIT: usize = 200;

/// Per-call upper bound for `getMultipleAccounts`. Most public Solana RPC
/// providers reject requests with more than 100 keys; we chunk every batch
/// fetch in the squads module by this constant.
pub(crate) const MAX_GET_ACCOUNTS: usize = 100;

/// Concurrency for transaction fetches during vault resolution. RPC providers
/// vary in throughput; 20 is a balance between throughput and rate-limiting.
const VAULT_RESOLUTION_FETCH_CONCURRENCY: usize = 20;

/// Find the multisig that owns `vault` by scanning its recent transactions
/// for one that invoked a Squads program, then identifying the Multisig PDA
/// among the transaction's accounts by discriminator. Transactions are
/// fetched in parallel and candidate accounts are checked in batches via
/// `getMultipleAccounts`.
async fn resolve_vault_to_multisig<C: AsRef<SolanaRpcClient>>(
    client: &C,
    vault: &Pubkey,
) -> Result<Pubkey, Error> {
    let rpc = client.as_ref();
    let signatures = rpc
        .get_signatures_for_address_with_config(
            vault,
            GetConfirmedSignaturesForAddress2Config {
                limit: Some(VAULT_RESOLUTION_SCAN_LIMIT),
                commitment: Some(CommitmentConfig::confirmed()),
                ..Default::default()
            },
        )
        .await?;

    let signatures: Vec<Signature> = signatures
        .into_iter()
        .filter(|s| s.err.is_none())
        .filter_map(|s| Signature::from_str(&s.signature).ok())
        .collect();

    let txn_config = solana_client::rpc_config::RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::Json),
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
    };

    let mut tried: HashSet<Pubkey> = HashSet::new();
    let mut tx_stream = stream::iter(signatures)
        .map(|sig| async move { rpc.get_transaction_with_config(&sig, txn_config).await })
        .buffered(VAULT_RESOLUTION_FETCH_CONCURRENCY);

    while let Some(txn_result) = tx_stream.next().await {
        let Ok(txn) = txn_result else { continue };
        let keys = collect_account_keys(&txn);
        // Skip transactions that don't reference a Squads program at all —
        // they can't contain the multisig.
        if !keys
            .iter()
            .any(|k| *k == v3::PROGRAM_ID || *k == v4::PROGRAM_ID)
        {
            continue;
        }

        let candidates: Vec<Pubkey> = keys
            .into_iter()
            .filter(|k| !is_known_non_multisig(k, vault))
            .filter(|k| tried.insert(*k))
            .collect();
        if candidates.is_empty() {
            continue;
        }

        // Chunk to the 100-key `getMultipleAccounts` limit most public
        // RPC providers enforce. A LUT-heavy transaction can plausibly
        // produce >100 candidates after the filter; a single-shot call
        // would be rejected mid-resolution.
        for chunk in candidates.chunks(MAX_GET_ACCOUNTS) {
            let accounts = raw_get_multiple_accounts(&rpc.url(), chunk).await?;
            for (candidate, account) in chunk.iter().zip(accounts) {
                let Some(account) = account else { continue };
                if v4::is_multisig_account(&account.owner, &account.data)
                    || v3::is_multisig_account(&account.owner, &account.data)
                {
                    return Ok(*candidate);
                }
            }
        }
    }

    Err(SquadsError::vault_resolution_failed(*vault, VAULT_RESOLUTION_SCAN_LIMIT).into())
}

/// Pre-filter candidates that obviously can't be the multisig: the vault
/// itself and any program / common SPL utility account we already recognize.
/// Cuts wasted `getMultipleAccounts` work — multisig PDAs are never one of
/// these.
fn is_known_non_multisig(key: &Pubkey, vault: &Pubkey) -> bool {
    *key == *vault || KnownProgram::from_pubkey(key).is_some()
}

/// Just enough of an account to check ownership and discriminator.
pub(crate) struct RawAccount {
    pub owner: Pubkey,
    pub data: Vec<u8>,
}

/// `getMultipleAccounts` via raw JSON-RPC, bypassing solana-client's response
/// decoder. The official decoder fails on accounts with `rent_epoch = u64::MAX`
/// (the rent-exempt sentinel) because the RPC encodes that value as a JSON
/// float that overflows the strict u64 deserializer. We don't need
/// `rent_epoch` at all, so we parse only the fields we use.
pub(crate) async fn raw_get_multiple_accounts(
    rpc_url: &str,
    keys: &[Pubkey],
) -> Result<Vec<Option<RawAccount>>, Error> {
    use base64::{engine::general_purpose::STANDARD, Engine};

    #[derive(serde::Deserialize)]
    struct AccountInfo {
        owner: String,
        data: (String, String), // (base64 payload, encoding tag)
    }
    #[derive(serde::Deserialize)]
    struct Value {
        value: Vec<Option<AccountInfo>>,
    }
    #[derive(serde::Deserialize)]
    struct RpcError {
        code: i64,
        message: String,
    }
    #[derive(serde::Deserialize)]
    struct Response {
        // Successful responses carry `result`; error responses carry
        // `error` instead. Both fields are optional so we can surface
        // the RPC's own error message rather than decoding-only fields
        // and bubbling up "missing field `result`".
        result: Option<Value>,
        error: Option<RpcError>,
    }

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getMultipleAccounts",
        "params": [
            keys.iter().map(ToString::to_string).collect::<Vec<_>>(),
            { "encoding": "base64", "commitment": "confirmed" },
        ],
    });
    let resp: Response = reqwest::Client::new()
        .post(rpc_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if let Some(err) = resp.error {
        return Err(SquadsError::rpc("getMultipleAccounts", err.code, err.message).into());
    }
    let value = resp
        .result
        .ok_or_else(|| SquadsError::malformed_rpc("getMultipleAccounts"))?;

    value
        .value
        .into_iter()
        .map(|maybe| match maybe {
            None => Ok(None),
            Some(info) => {
                let owner = Pubkey::from_str(&info.owner).map_err(DecodeError::from)?;
                let data = STANDARD.decode(info.data.0).map_err(DecodeError::from)?;
                Ok(Some(RawAccount { owner, data }))
            }
        })
        .collect()
}

/// All account keys involved in a transaction, including those resolved via
/// address lookup tables (which `VersionedTransaction::static_account_keys`
/// alone misses).
fn collect_account_keys(
    txn: &solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta,
) -> Vec<Pubkey> {
    let mut out = Vec::new();
    if let solana_transaction_status::EncodedTransaction::Json(ui_tx) = &txn.transaction.transaction
    {
        if let solana_transaction_status::UiMessage::Raw(raw) = &ui_tx.message {
            out.extend(
                raw.account_keys
                    .iter()
                    .filter_map(|s| Pubkey::from_str(s).ok()),
            );
        }
    }
    if let Some(meta) = &txn.transaction.meta {
        if let OptionSerializer::Some(UiLoadedAddresses { writable, readonly }) =
            &meta.loaded_addresses
        {
            out.extend(writable.iter().filter_map(|s| Pubkey::from_str(s).ok()));
            out.extend(readonly.iter().filter_map(|s| Pubkey::from_str(s).ok()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs::squads_mpl;

    /// Construct a fully-shaped v3 `MsTransaction` body so
    /// `extract_target` can deserialize it end-to-end. The Active
    /// status, empty vote vecs, and zeroed bumps are placeholders —
    /// classify_target only reads `ms` and `transaction_index`.
    fn v3_ms_transaction_body(multisig: Pubkey, index: u32) -> Vec<u8> {
        use anchor_lang::AnchorSerialize;
        let mut body = v3::MS_TRANSACTION_DISCRIMINATOR.to_vec();
        let tx = squads_mpl::accounts::MsTransaction {
            creator: Pubkey::new_unique(),
            ms: multisig,
            transaction_index: index,
            authority_index: 0,
            authority_bump: 0,
            status: squads_mpl::types::MsTransactionStatus::Active,
            instruction_index: 0,
            bump: 0,
            approved: vec![],
            rejected: vec![],
            cancelled: vec![],
            executed_index: 0,
        };
        tx.serialize(&mut body).unwrap();
        body
    }

    /// `classify_target` dispatches by account owner and (for
    /// Squads-owned accounts) the body discriminator. Pins each arm:
    /// v4 owner + multisig disc → Multisig{V4}; v3 owner + multisig
    /// disc → Multisig{V3}; v3 owner + valid MsTransaction body →
    /// Transaction{V3, multisig, index}; system-program-owned → Vault;
    /// unknown owner → Err.
    ///
    /// Skips constructing a synthetic v4 VaultTransaction body — the
    /// upstream `squads-multisig-program::state::VaultTransaction`
    /// has a long field list and we'd be testing Borsh decoding more
    /// than dispatch. The v3 MsTransaction case exercises the
    /// "Transaction" arm; v4's equivalent dispatch is the same shape.
    #[test]
    fn classify_target_dispatch() {
        let target = Pubkey::new_unique();
        let multisig = Pubkey::new_unique();

        // v4 Multisig discriminator returns Multisig{V4} without
        // attempting body decode.
        let body = v4::MULTISIG_DISCRIMINATOR.to_vec();
        match classify_target(&target, &v4::PROGRAM_ID, &body).unwrap() {
            TargetKind::Multisig {
                version: Version::V4,
            } => {}
            other => panic!("expected Multisig {{V4}}, got {other:?}"),
        }

        // v3 Ms discriminator → Multisig{V3}.
        let body = v3::MS_DISCRIMINATOR.to_vec();
        match classify_target(&target, &v3::PROGRAM_ID, &body).unwrap() {
            TargetKind::Multisig {
                version: Version::V3,
            } => {}
            other => panic!("expected Multisig {{V3}}, got {other:?}"),
        }

        // v3 MsTransaction with a valid body → Transaction{V3, ...}.
        let body = v3_ms_transaction_body(multisig, 7);
        match classify_target(&target, &v3::PROGRAM_ID, &body).unwrap() {
            TargetKind::Transaction {
                version: Version::V3,
                multisig: m,
                index,
            } => {
                assert_eq!(m.into_pubkey(), multisig);
                assert_eq!(index, 7);
            }
            other => panic!("expected Transaction {{V3}}, got {other:?}"),
        }

        // System-program-owned (vault). Body is irrelevant.
        match classify_target(&target, &solana_sdk::system_program::ID, &[]).unwrap() {
            TargetKind::Vault => {}
            other => panic!("expected Vault, got {other:?}"),
        }

        // Unknown owner → Err.
        let stranger = Pubkey::new_unique();
        assert!(classify_target(&target, &stranger, &[]).is_err());
    }

    /// `validate_membership` returns Ok when the wallet is a member with
    /// the required permission, NotAMember when not in the list, and
    /// MissingPermission when in the list but lacking the action's
    /// permission bit. v3-style ALL-permissions members always pass.
    #[test]
    fn validate_membership_paths() {
        use super::error::{SquadsError, SquadsMembershipError};

        let multisig_pk = Pubkey::new_unique();
        let multisig = MultisigKey::from_pubkey(multisig_pk);
        let alice = Pubkey::new_unique();
        let bob = Pubkey::new_unique();
        let stranger = Pubkey::new_unique();

        let info = MultisigInfo {
            address: multisig,
            version: Version::V4,
            threshold: 1,
            transaction_index: 0,
            resolved_from_vault: None,
            members: vec![
                MemberInfo {
                    key: alice,
                    permissions: MemberPermissions::ALL,
                },
                MemberInfo {
                    key: bob,
                    // Bob can vote but not initiate or execute.
                    permissions: MemberPermissions {
                        propose: false,
                        vote: true,
                        execute: false,
                    },
                },
            ],
        };

        // Alice has all permissions — every action passes.
        for action in [
            MemberAction::Vote,
            MemberAction::Execute,
            MemberAction::Initiate,
        ] {
            assert!(
                validate_membership(&info, &alice, action).is_ok(),
                "alice should pass {action:?}",
            );
        }

        // Bob can vote but not initiate or execute.
        assert!(validate_membership(&info, &bob, MemberAction::Vote).is_ok());
        match validate_membership(&info, &bob, MemberAction::Initiate).unwrap_err() {
            Error::Squads(SquadsError::Membership(SquadsMembershipError::MissingPermission {
                wallet,
                multisig: m,
                action,
            })) => {
                assert_eq!(wallet, bob);
                assert_eq!(m, multisig_pk);
                assert_eq!(action, "initiate");
            }
            other => panic!("expected MissingPermission, got {other:?}"),
        }
        match validate_membership(&info, &bob, MemberAction::Execute).unwrap_err() {
            Error::Squads(SquadsError::Membership(SquadsMembershipError::MissingPermission {
                action,
                ..
            })) => {
                assert_eq!(action, "execute");
            }
            other => panic!("expected MissingPermission, got {other:?}"),
        }

        // Stranger isn't a member at all.
        match validate_membership(&info, &stranger, MemberAction::Vote).unwrap_err() {
            Error::Squads(SquadsError::Membership(SquadsMembershipError::NotAMember {
                wallet,
                multisig: m,
            })) => {
                assert_eq!(wallet, stranger);
                assert_eq!(m, multisig_pk);
            }
            other => panic!("expected NotAMember, got {other:?}"),
        }
    }

    /// `ProposalView` serializes with an internally-tagged `version`
    /// field so consumers can route on `v3` / `v4` rather than sniffing
    /// for shape-disjoint fields. Pins the on-the-wire JSON shape on
    /// both arms so a future tag-removal regression is loud rather
    /// than silent.
    #[test]
    fn proposal_view_serializes_with_version_tag() {
        // v3 arm.
        let summary = build_summary("0/1".to_string(), false, &[]);
        let v3_view = ProposalView::V3(v3::ProposalInfo {
            summary,
            multisig: MultisigKey::from_pubkey(Pubkey::new_unique()),
            transaction_index: 1,
            transaction: Pubkey::new_unique(),
            status: v3::ProposalStatusInfo::Active,
            votes: ProposalVotes {
                approved: 0,
                rejected: 0,
                cancelled: 0,
            },
            authority_index: 0,
            authority: VaultKey::from_pubkey(Pubkey::new_unique()),
            creator: Pubkey::new_unique(),
            instructions: vec![],
        });
        let json = serde_json::to_value(&v3_view).unwrap();
        assert_eq!(
            json.get("version").and_then(|v| v.as_str()),
            Some("v3"),
            "v3 ProposalView must carry version=\"v3\"; got {json}",
        );

        // v4 arm — paired so a regression that drops the tag from
        // only one variant gets caught here too. Use the simplest v4
        // ProposalInfo variant (ConfigTransaction) so the test
        // doesn't rely on lookup-table resolution machinery.
        let v4_summary = build_summary("0/1".to_string(), false, &[]);
        let v4_view = ProposalView::V4(v4::ProposalInfo::ConfigTransaction(
            v4::ConfigTransactionInfo {
                summary: v4_summary,
                multisig: MultisigKey::from_pubkey(Pubkey::new_unique()),
                transaction_index: 1,
                proposal: Pubkey::new_unique(),
                config_transaction: Pubkey::new_unique(),
                status: v4::ProposalStatusInfo::Executing,
                votes: ProposalVotes {
                    approved: 0,
                    rejected: 0,
                    cancelled: 0,
                },
                creator: Pubkey::new_unique(),
                actions: vec![],
            },
        ));
        let json = serde_json::to_value(&v4_view).unwrap();
        assert_eq!(
            json.get("version").and_then(|v| v.as_str()),
            Some("v4"),
            "v4 ProposalView must carry version=\"v4\"; got {json}",
        );
    }
}
