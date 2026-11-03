//! Squads v3 (`SMPLecH…`). Account types come from the on-chain IDL via
//! `declare_program!(squads_mpl)`. The `squads-mpl` crate is archived on a
//! solana 1.x toolchain and unusable as a direct dep; the IDL path works
//! because v3 has no parameterized types (unlike v4's `SmallVec<L,T>`).
//!
//! v3's transaction model is split across multiple accounts: an
//! `MsTransaction` (status / metadata) plus one `MsInstruction` PDA per
//! inner instruction. To inspect a proposal we fetch the transaction then
//! enumerate its instruction PDAs in parallel.

use super::{
    InstructionAccountRef, InstructionInfo, MemberInfo, MemberPermissions, MultisigInfo,
    MultisigKey, ProposalSummary, ProposalVotes, VaultKey, Version,
};
use crate::{
    client::SolanaRpcClient,
    error::{DecodeError, Error},
    keypair::Pubkey,
    programs::KnownProgram,
};
use anchor_lang::AccountDeserialize;
use futures::stream::{self, StreamExt, TryStreamExt};
use serde::Serialize;
use solana_sdk::{bs58, pubkey};

pub use crate::programs::squads_mpl;

pub const PROGRAM_ID: Pubkey = pubkey!("SMPLecH534NA9acpos4G6x7uf3LWbCAwZQE9e8ZekMu");

const SEED_PREFIX: &[u8] = b"squad";
const SEED_TRANSACTION: &[u8] = b"transaction";
const SEED_INSTRUCTION: &[u8] = b"instruction";
const SEED_AUTHORITY: &[u8] = b"authority";

/// First 8 bytes of `sha256("account:<TypeName>")` — Anchor's standard
/// account discriminators. Used for cheap owner+discriminator checks
/// during vault resolution; full deserialize is reserved for the
/// fetch-and-decode paths.
pub(super) const MS_DISCRIMINATOR: [u8; 8] = [70, 118, 9, 108, 254, 215, 31, 120];
pub(super) const MS_TRANSACTION_DISCRIMINATOR: [u8; 8] = [182, 151, 104, 216, 255, 1, 19, 157];

/// Derive the `MsTransaction` PDA for a given (multisig, transaction index)
/// pair. v3 transaction indices are u32 (not u64 like v4).
pub fn transaction_pda(multisig: &MultisigKey, transaction_index: u32) -> Pubkey {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            multisig.as_ref(),
            &transaction_index.to_le_bytes(),
            SEED_TRANSACTION,
        ],
        &PROGRAM_ID,
    )
    .0
}

/// Derive the `MsInstruction` PDA for a given (transaction, instruction
/// index) pair. v3 instruction indices are 1-based u8.
pub fn instruction_pda(transaction: &Pubkey, instruction_index: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            transaction.as_ref(),
            &instruction_index.to_le_bytes(),
            SEED_INSTRUCTION,
        ],
        &PROGRAM_ID,
    )
    .0
}

/// Derive the authority (vault) PDA for a given (multisig, authority index)
/// pair. v3 calls these "authorities"; functionally they're the same as
/// v4's vaults — system-owned PDAs that hold funds and act as signers.
pub fn authority_pda(multisig: &MultisigKey, authority_index: u32) -> VaultKey {
    let pk = Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            multisig.as_ref(),
            &authority_index.to_le_bytes(),
            SEED_AUTHORITY,
        ],
        &PROGRAM_ID,
    )
    .0;
    VaultKey::from_pubkey(pk)
}

/// Decoded view of a v3 transaction (proposal + the instructions it will
/// execute). Mirrors `v4::ProposalInfo` where the concepts overlap;
/// version-specific bits stay distinct (v3 has no per-status timestamps,
/// no LUTs, no `stale_transaction_index` cross-check).
#[derive(Debug, Clone, Serialize)]
pub struct ProposalInfo {
    pub summary: ProposalSummary,
    pub multisig: MultisigKey,
    pub transaction_index: u32,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub transaction: Pubkey,
    pub status: ProposalStatusInfo,
    pub votes: ProposalVotes,
    pub authority_index: u32,
    /// The "vault" in v3 terms — the system-owned PDA that signs CPI calls
    /// on behalf of the multisig.
    pub authority: VaultKey,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub creator: Pubkey,
    pub instructions: Vec<InstructionInfo>,
}

/// v3 status enum. Unlike v4, v3 doesn't record a timestamp per status.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatusInfo {
    Draft,
    Active,
    /// Threshold met, ready for `execute_transaction`.
    ExecuteReady,
    Executed,
    Rejected,
    Cancelled,
}

/// List open proposals on a v3 multisig in one bulk fetch. Scans
/// `1..=transaction_index` and filters to non-finalized statuses
/// (Draft, Active, ExecuteReady). v3 has no `stale_transaction_index`
/// equivalent, so even old proposals against a since-modified multisig
/// can still appear; the on-chain `execute_transaction` path enforces
/// the staleness check itself.
pub async fn list_open_proposals<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: &MultisigKey,
) -> Result<Vec<super::ProposalListEntry>, Error> {
    let rpc = client.as_ref();
    let ms =
        fetch_account::<squads_mpl::accounts::Ms>(rpc, multisig.as_pubkey(), "v3 Multisig").await?;
    let last = ms.transaction_index;
    if last == 0 {
        return Ok(Vec::new());
    }

    let indices: Vec<u32> = (1..=last).collect();
    let pdas: Vec<Pubkey> = indices
        .iter()
        .map(|i| transaction_pda(multisig, *i))
        .collect();

    let mut entries: Vec<super::ProposalListEntry> = Vec::new();
    let rpc_url = rpc.url();
    for (idx_chunk, pda_chunk) in indices
        .chunks(super::MAX_GET_ACCOUNTS)
        .zip(pdas.chunks(super::MAX_GET_ACCOUNTS))
    {
        let accounts = super::raw_get_multiple_accounts(&rpc_url, pda_chunk).await?;
        for ((idx, pda), maybe_account) in idx_chunk.iter().zip(pda_chunk).zip(accounts) {
            let Some(account) = maybe_account else {
                continue;
            };
            // Owner mismatch / wrong discriminator means the PDA holds
            // something other than an MsTransaction — skip silently
            // (matches v4's filter behaviour). A discriminator-matched
            // body that fails to decode is corruption worth surfacing.
            if account.owner != PROGRAM_ID
                || account.data.len() < 8
                || account.data[..8] != MS_TRANSACTION_DISCRIMINATOR
            {
                continue;
            }
            let tx = squads_mpl::accounts::MsTransaction::try_deserialize(&mut &account.data[..])
                .map_err(|e| DecodeError::deserialize(pda, "v3 MsTransaction", e))?;
            let Some(status) = open_status_label(&tx.status) else {
                continue;
            };
            entries.push(super::ProposalListEntry {
                index: u64::from(*idx),
                transaction: *pda,
                status,
                // v3's MsTransaction doesn't carry per-status
                // timestamps. The Squads UI synthesizes them from
                // signature history; we'd need a per-row
                // `getSignaturesForAddress` to do the same, which
                // isn't worth the latency for a list view.
                status_timestamp: None,
                votes: ProposalVotes {
                    approved: tx.approved.len(),
                    rejected: tx.rejected.len(),
                    cancelled: tx.cancelled.len(),
                },
            });
        }
    }
    // Newest first — matches the Squads UI's reverse-chronological
    // ordering and v4's `list_open_proposals`.
    entries.reverse();
    Ok(entries)
}

fn open_status_label(status: &squads_mpl::types::MsTransactionStatus) -> Option<&'static str> {
    use squads_mpl::types::MsTransactionStatus;
    match status {
        MsTransactionStatus::Draft => Some("draft"),
        MsTransactionStatus::Active => Some("active"),
        MsTransactionStatus::ExecuteReady => Some("execute_ready"),
        MsTransactionStatus::Executed
        | MsTransactionStatus::Rejected
        | MsTransactionStatus::Cancelled => None,
    }
}

/// Fetch a v3 transaction and all its instruction accounts and produce
/// a reviewer-friendly summary. `multisig_or_vault` accepts whatever
/// `super::resolve_to_multisig` accepts: a v3 Ms PDA, an authority
/// (vault) PDA (resolved through the cache + scan path), or an
/// MsTransaction PDA — multisig is read from the body.
pub async fn get_proposal_info<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig_or_vault: &Pubkey,
    transaction_index: u32,
) -> Result<ProposalInfo, Error> {
    let multisig_addr = super::resolve_to_multisig(client, multisig_or_vault).await?;
    let transaction_addr = transaction_pda(&multisig_addr, transaction_index);

    let rpc = client.as_ref();
    let multisig =
        fetch_account::<squads_mpl::accounts::Ms>(rpc, multisig_addr.as_pubkey(), "v3 Multisig")
            .await?;
    let transaction = fetch_account::<squads_mpl::accounts::MsTransaction>(
        rpc,
        &transaction_addr,
        "v3 MsTransaction",
    )
    .await?;

    // Instruction PDAs are 1-indexed and dense; a transaction with
    // `instruction_index = N` has PDAs at seeds 1..=N. Fetch in parallel.
    let instruction_addrs: Vec<Pubkey> = (1..=transaction.instruction_index)
        .map(|i| instruction_pda(&transaction_addr, i))
        .collect();
    let instructions: Vec<InstructionInfo> = stream::iter(instruction_addrs)
        .map(|addr| async move {
            let ms_ix = fetch_account::<squads_mpl::accounts::MsInstruction>(
                rpc,
                &addr,
                "v3 MsInstruction",
            )
            .await?;
            Ok::<InstructionInfo, Error>(ms_instruction_to_info(&ms_ix))
        })
        .buffered(10)
        .try_collect()
        .await?;

    let authority = authority_pda(&multisig_addr, transaction.authority_index);
    let summary = build_summary(&transaction, multisig.threshold, &instructions);
    let status = convert_status(&transaction.status);

    Ok(ProposalInfo {
        summary,
        multisig: multisig_addr,
        transaction_index: transaction.transaction_index,
        transaction: transaction_addr,
        status,
        votes: ProposalVotes {
            approved: transaction.approved.len(),
            rejected: transaction.rejected.len(),
            cancelled: transaction.cancelled.len(),
        },
        authority_index: transaction.authority_index,
        authority,
        creator: transaction.creator,
        instructions,
    })
}

pub(super) fn decode_multisig(
    address: MultisigKey,
    data: &[u8],
    resolved_from_vault: Option<VaultKey>,
) -> Result<MultisigInfo, Error> {
    let ms = squads_mpl::accounts::Ms::try_deserialize(&mut &data[..])
        .map_err(|e| DecodeError::deserialize(address.as_pubkey(), "v3 Multisig", e))?;
    let members = ms
        .keys
        .iter()
        .map(|k| MemberInfo {
            key: *k,
            permissions: MemberPermissions::ALL,
        })
        .collect();
    Ok(MultisigInfo {
        address,
        version: Version::V3,
        threshold: ms.threshold,
        transaction_index: u64::from(ms.transaction_index),
        members,
        resolved_from_vault,
    })
}

/// Cheap check for "this account is a v3 Multisig" used during vault
/// resolution scans. Validates owner and discriminator without
/// committing to a full deserialize.
pub(super) fn is_multisig_account(owner: &Pubkey, data: &[u8]) -> bool {
    *owner == PROGRAM_ID && data.len() >= 8 && data[..8] == MS_DISCRIMINATOR
}

/// Self-identify a v3 account that already passed the owner check. Returns
/// `Ok(Some((ms, index)))` if the account is an `MsTransaction`; `Ok(None)`
/// if it's an `Ms` (multisig — caller supplies the index); `Err` if the
/// discriminator isn't recognized. v3 transaction indices are u32 on chain
/// and widen to u64 for the unified API.
pub(super) fn extract_target(
    address: &Pubkey,
    data: &[u8],
) -> Result<Option<(Pubkey, u64)>, Error> {
    let disc = super::read_discriminator(data)
        .ok_or_else(|| DecodeError::wrong_discriminator(address, "v3 Ms or MsTransaction"))?;
    match disc {
        MS_DISCRIMINATOR => Ok(None),
        MS_TRANSACTION_DISCRIMINATOR => {
            let tx = squads_mpl::accounts::MsTransaction::try_deserialize(&mut &data[..])
                .map_err(|e| DecodeError::deserialize(address, "v3 MsTransaction", e))?;
            Ok(Some((tx.ms, u64::from(tx.transaction_index))))
        }
        _ => Err(DecodeError::wrong_discriminator(address, "v3 Ms or MsTransaction").into()),
    }
}

/// Fetch and decode a v3 Anchor account. The 0.31 `AccountDeserialize`
/// trait checks the 8-byte discriminator internally, so wrong-account-type
/// failures surface here as a single deserialize error.
async fn fetch_account<T: AccountDeserialize>(
    rpc: &SolanaRpcClient,
    address: &Pubkey,
    type_name: &'static str,
) -> Result<T, Error> {
    let account = rpc.get_account(address).await?;
    if account.owner != PROGRAM_ID {
        return Err(DecodeError::wrong_owner(address, "Squads v3", &account.owner).into());
    }
    T::try_deserialize(&mut &account.data[..])
        .map_err(|e| DecodeError::deserialize(address, type_name, e).into())
}

fn ms_instruction_to_info(ix: &squads_mpl::accounts::MsInstruction) -> InstructionInfo {
    let accounts = ix
        .keys
        .iter()
        .map(|meta| InstructionAccountRef {
            pubkey: meta.pubkey,
            writable: meta.is_writable,
            signer: meta.is_signer,
        })
        .collect();
    let program = KnownProgram::from_pubkey(&ix.program_id);
    let disc_bytes = super::read_discriminator(&ix.data);
    let body = ix.data.get(8..);
    let method = program
        .zip(disc_bytes.as_ref())
        .and_then(|(p, d)| p.method_name_with_body(d, body.unwrap_or(&[])));
    let args = program
        .zip(disc_bytes.as_ref())
        .zip(body)
        .and_then(|((p, d), b)| p.decode_instruction_args(d, b));
    let discriminator = disc_bytes.map(|d| bs58::encode(d).into_string());
    InstructionInfo {
        program_id: ix.program_id,
        program,
        method,
        args,
        accounts,
        data_len: ix.data.len(),
        discriminator,
        data_b58: bs58::encode(&ix.data).into_string(),
    }
}

fn convert_status(status: &squads_mpl::types::MsTransactionStatus) -> ProposalStatusInfo {
    use squads_mpl::types::MsTransactionStatus;
    match status {
        MsTransactionStatus::Draft => ProposalStatusInfo::Draft,
        MsTransactionStatus::Active => ProposalStatusInfo::Active,
        MsTransactionStatus::ExecuteReady => ProposalStatusInfo::ExecuteReady,
        MsTransactionStatus::Executed => ProposalStatusInfo::Executed,
        MsTransactionStatus::Rejected => ProposalStatusInfo::Rejected,
        MsTransactionStatus::Cancelled => ProposalStatusInfo::Cancelled,
    }
}

fn build_summary(
    transaction: &squads_mpl::accounts::MsTransaction,
    threshold: u16,
    instructions: &[InstructionInfo],
) -> ProposalSummary {
    let approved = transaction.approved.len();
    let approvals = format!("{approved}/{threshold}");
    // v3 has no per-transaction stale flag (it tracks `ms_change_index` on
    // the multisig but doesn't record it on the transaction, so we can't
    // tell from the transaction alone whether settings changed since it
    // was created). Always reports `false`.
    super::build_summary(approvals, false, instructions)
}
