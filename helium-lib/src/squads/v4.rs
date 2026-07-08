//! Squads v4 (`SQDS4ep…`). Types come from the upstream
//! `squads-multisig-program` crate.

use super::{
    error::{CompiledInstructionField, MessageField, SquadsError},
    InstructionAccountRef, InstructionInfo, MemberInfo, MemberPermissions, MultisigInfo,
    MultisigKey, ProposalSummary, ProposalVotes, VaultKey, Version,
};
use crate::{
    client::SolanaRpcClient,
    error::{DecodeError, Error},
    keypair::Pubkey,
    programs::KnownProgram,
};
use anchor_lang::prelude::AnchorDeserialize;
use chrono::{DateTime, Utc};
use futures::stream::{StreamExt, TryStreamExt};
use serde::Serialize;
use solana_sdk::{bs58, pubkey};

pub const PROGRAM_ID: Pubkey = pubkey!("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");

/// First 8 bytes of `sha256("account:<TypeName>")` — Anchor's standard
/// account discriminators. Hardcoded because the upstream types are from a
/// different anchor-lang version than helium-lib's, so we can't reuse the
/// trait-generated constants directly.
pub(super) const MULTISIG_DISCRIMINATOR: [u8; 8] = [224, 116, 121, 186, 68, 161, 79, 236];
const PROPOSAL_DISCRIMINATOR: [u8; 8] = [26, 94, 189, 187, 116, 136, 53, 33];
pub(super) const VAULT_TRANSACTION_DISCRIMINATOR: [u8; 8] = [168, 250, 162, 100, 81, 14, 162, 207];
const CONFIG_TRANSACTION_DISCRIMINATOR: [u8; 8] = [94, 8, 4, 35, 113, 139, 139, 112];
const BATCH_DISCRIMINATOR: [u8; 8] = [156, 194, 70, 44, 22, 88, 137, 44];
const VAULT_BATCH_TRANSACTION_DISCRIMINATOR: [u8; 8] = [196, 121, 46, 36, 12, 19, 252, 7];

/// Squads v4 PDA seed prefixes.
const SEED_PREFIX: &[u8] = b"multisig";
const SEED_TRANSACTION: &[u8] = b"transaction";
const SEED_PROPOSAL: &[u8] = b"proposal";
const SEED_VAULT: &[u8] = b"vault";
const SEED_BATCH_TRANSACTION: &[u8] = b"batch_transaction";

/// Derive the `VaultTransaction` PDA for a given (multisig, transaction
/// index) pair.
pub fn vault_transaction_pda(multisig: &MultisigKey, transaction_index: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            multisig.as_ref(),
            SEED_TRANSACTION,
            &transaction_index.to_le_bytes(),
        ],
        &PROGRAM_ID,
    )
    .0
}

/// Derive the `Proposal` PDA for a given (multisig, transaction index) pair.
pub fn proposal_pda(multisig: &MultisigKey, transaction_index: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            multisig.as_ref(),
            SEED_TRANSACTION,
            &transaction_index.to_le_bytes(),
            SEED_PROPOSAL,
        ],
        &PROGRAM_ID,
    )
    .0
}

/// Derive the `Vault` PDA for a given (multisig, vault index) pair.
pub fn vault_pda(multisig: &MultisigKey, vault_index: u8) -> VaultKey {
    let pk = Pubkey::find_program_address(
        &[SEED_PREFIX, multisig.as_ref(), SEED_VAULT, &[vault_index]],
        &PROGRAM_ID,
    )
    .0;
    VaultKey::from_pubkey(pk)
}

/// Derive a sub-transaction PDA inside a Batch. `sub_index` is 1-based
/// and dense — a Batch with `size = N` has VaultBatchTransaction PDAs at
/// indexes `1..=N`. Note the index is u32 LE (matches Squads' on-chain
/// `batch.size`) while the parent `batch_index` is u64 LE.
pub fn batch_transaction_pda(multisig: &MultisigKey, batch_index: u64, sub_index: u32) -> Pubkey {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            multisig.as_ref(),
            SEED_TRANSACTION,
            &batch_index.to_le_bytes(),
            SEED_BATCH_TRANSACTION,
            &sub_index.to_le_bytes(),
        ],
        &PROGRAM_ID,
    )
    .0
}

/// Decoded view of a v4 proposal. v4 supports three transaction kinds —
/// `VaultTransaction` (the common one: arbitrary instructions executed
/// by the vault), `ConfigTransaction` (changes to the multisig itself:
/// members, threshold, time-lock, spending limits), and `Batch` (a
/// serial sequence of vault transactions). The kind is dispatched from
/// the transaction account's Anchor discriminator.
///
/// `summary` is rendered first so reviewers see the load-bearing facts
/// (approvals vs threshold, staleness) before scrolling through detail.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalInfo {
    VaultTransaction(VaultTransactionInfo),
    ConfigTransaction(ConfigTransactionInfo),
    /// Serial execution of multiple vault transactions. Sub-transactions
    /// are decoded with the same LUT-resolved account treatment as a
    /// top-level VaultTransaction (see `BatchInfo.sub_transactions`).
    Batch(BatchInfo),
}

#[derive(Debug, Clone, Serialize)]
pub struct VaultTransactionInfo {
    pub summary: ProposalSummary,
    pub multisig: MultisigKey,
    pub transaction_index: u64,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub proposal: Pubkey,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub vault_transaction: Pubkey,
    pub status: ProposalStatusInfo,
    pub votes: ProposalVotes,
    pub vault_index: u8,
    pub vault: VaultKey,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub creator: Pubkey,
    pub ephemeral_signers: u8,
    pub instructions: Vec<InstructionInfo>,
    pub address_lookup_tables: Vec<AddressLookupInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigTransactionInfo {
    pub summary: ProposalSummary,
    pub multisig: MultisigKey,
    pub transaction_index: u64,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub proposal: Pubkey,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub config_transaction: Pubkey,
    pub status: ProposalStatusInfo,
    pub votes: ProposalVotes,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub creator: Pubkey,
    /// Ordered list of multisig-config changes this proposal will apply
    /// when executed. Each action is a single mutation — add member,
    /// change threshold, etc.
    pub actions: Vec<ConfigActionInfo>,
}

/// Reviewer-friendly rendering of a `ConfigAction`. Uses an internally
/// tagged enum so each action's parameters live alongside the action's
/// `type` in the JSON output.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigActionInfo {
    AddMember {
        #[serde(with = "crate::keypair::serde_pubkey")]
        new_member: Pubkey,
        permissions: MemberPermissions,
    },
    RemoveMember {
        #[serde(with = "crate::keypair::serde_pubkey")]
        old_member: Pubkey,
    },
    ChangeThreshold {
        new_threshold: u16,
    },
    SetTimeLock {
        new_time_lock: u32,
    },
    AddSpendingLimit {
        #[serde(with = "crate::keypair::serde_pubkey")]
        create_key: Pubkey,
        vault_index: u8,
        #[serde(with = "crate::keypair::serde_pubkey")]
        mint: Pubkey,
        amount: u64,
        period: ConfigSpendingPeriod,
        members: Vec<String>,
        destinations: Vec<String>,
    },
    RemoveSpendingLimit {
        #[serde(with = "crate::keypair::serde_pubkey")]
        spending_limit: Pubkey,
    },
    SetRentCollector {
        #[serde(with = "crate::keypair::serde_opt_pubkey")]
        new_rent_collector: Option<Pubkey>,
    },
    /// `ConfigAction` is `#[non_exhaustive]` upstream; future variants
    /// land here until we update.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchInfo {
    pub summary: ProposalSummary,
    pub multisig: MultisigKey,
    pub transaction_index: u64,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub proposal: Pubkey,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub batch: Pubkey,
    pub status: ProposalStatusInfo,
    pub votes: ProposalVotes,
    pub vault_index: u8,
    pub vault: VaultKey,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub creator: Pubkey,
    /// Number of vault transactions in the batch.
    pub size: u32,
    /// How many sub-transactions have already been executed (0..=size).
    pub executed_transactions: u32,
    /// Decoded sub-transactions in execution order (1-based on chain;
    /// 0-indexed in this `Vec`).
    pub sub_transactions: Vec<BatchSubTransaction>,
}

/// One sub-transaction inside a Batch — same instruction shape as a
/// VaultTransaction but stored as a separate `VaultBatchTransaction` PDA.
/// The parent Batch already carries multisig/creator/vault_index, so each
/// sub-transaction only repeats what's specific to it.
#[derive(Debug, Clone, Serialize)]
pub struct BatchSubTransaction {
    /// 1-based position within the batch.
    pub batch_index: u32,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub address: Pubkey,
    pub ephemeral_signers: u8,
    pub instructions: Vec<InstructionInfo>,
    pub address_lookup_tables: Vec<AddressLookupInfo>,
}

/// v4 spending-limit reset period.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSpendingPeriod {
    OneTime,
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ProposalStatusInfo {
    Draft {
        timestamp: DateTime<Utc>,
    },
    Active {
        timestamp: DateTime<Utc>,
    },
    Rejected {
        timestamp: DateTime<Utc>,
    },
    Approved {
        timestamp: DateTime<Utc>,
    },
    Executed {
        timestamp: DateTime<Utc>,
    },
    Cancelled {
        timestamp: DateTime<Utc>,
    },
    /// Two cases collapse here: (a) the deprecated transient status
    /// preserved on old-format Squads accounts, and (b) any future
    /// variant of upstream `ProposalStatus` we haven't taught the
    /// converter about — `convert_proposal_status` falls through to
    /// `Executing` so a forward-incompat decode renders as something
    /// rather than failing the whole inspect.
    Executing,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddressLookupInfo {
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub address: Pubkey,
    pub writable_indexes: Vec<u8>,
    pub readonly_indexes: Vec<u8>,
}

/// List open proposals on a v4 multisig in one bulk fetch. Scans
/// `(stale_transaction_index + 1)..=transaction_index` — the only range
/// where votes/executes can still affect state — and filters to
/// non-finalized proposal statuses. The multisig address must already
/// be resolved (no vault → multisig translation here; the dispatch
/// happens at `super::list_open_proposals`).
pub async fn list_open_proposals<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: &MultisigKey,
) -> Result<Vec<super::ProposalListEntry>, Error> {
    use squads_multisig_program::state::Proposal;

    let rpc = client.as_ref();
    let multisig_state: squads_multisig_program::state::Multisig = fetch_account(
        rpc,
        multisig.as_pubkey(),
        &MULTISIG_DISCRIMINATOR,
        "v4 Multisig",
    )
    .await?;

    let stale = multisig_state.stale_transaction_index;
    let last = multisig_state.transaction_index;
    if last <= stale {
        return Ok(Vec::new());
    }

    let indices: Vec<u64> = (stale + 1..=last).collect();
    let pdas: Vec<Pubkey> = indices.iter().map(|i| proposal_pda(multisig, *i)).collect();

    let mut entries: Vec<super::ProposalListEntry> = Vec::new();
    let rpc_url = rpc.url();
    for (idx_chunk, pda_chunk) in indices
        .chunks(super::MAX_GET_ACCOUNTS)
        .zip(pdas.chunks(super::MAX_GET_ACCOUNTS))
    {
        let accounts = super::raw_get_multiple_accounts(&rpc_url, pda_chunk).await?;
        for ((idx, pda), maybe_account) in idx_chunk.iter().zip(pda_chunk).zip(accounts) {
            // Proposal PDAs aren't always allocated at every index — a
            // proposer can split `vault_transaction_create` and
            // `proposal_create` across separate transactions, leaving
            // gaps. Skip absent / mismatched accounts.
            let Some(account) = maybe_account else {
                continue;
            };
            if account.owner != PROGRAM_ID
                || account.data.len() < 8
                || account.data[..8] != PROPOSAL_DISCRIMINATOR
            {
                continue;
            }
            let proposal = Proposal::deserialize(&mut &account.data[8..])
                .map_err(|e| DecodeError::deserialize(pda, "v4 Proposal", e))?;
            let Some(status) = open_status_label(&proposal.status) else {
                continue;
            };
            entries.push(super::ProposalListEntry {
                index: *idx,
                transaction: vault_transaction_pda(multisig, *idx),
                status,
                status_timestamp: status_timestamp(&proposal.status),
                votes: votes_from(&proposal),
            });
        }
    }

    // Newest first — matches the Squads UI's reverse-chronological
    // ordering. Reviewers triage from the most-recent activity, so
    // putting old drafts at the bottom keeps the head of the list
    // useful.
    entries.reverse();
    Ok(entries)
}

/// Lift the timestamp out of any `ProposalStatus` variant. Returns
/// `None` only for the deprecated transient `Executing` (no
/// timestamp on chain) or if the on-chain seconds-since-epoch falls
/// outside chrono's range. Used to surface proposal age in
/// `list_open_proposals` without re-fetching the proposal account.
fn status_timestamp(
    status: &squads_multisig_program::state::ProposalStatus,
) -> Option<DateTime<Utc>> {
    use squads_multisig_program::state::ProposalStatus;
    let ts = match status {
        ProposalStatus::Draft { timestamp } => *timestamp,
        ProposalStatus::Active { timestamp } => *timestamp,
        ProposalStatus::Approved { timestamp } => *timestamp,
        ProposalStatus::Rejected { timestamp } => *timestamp,
        ProposalStatus::Executed { timestamp } => *timestamp,
        ProposalStatus::Cancelled { timestamp } => *timestamp,
        _ => return None,
    };
    DateTime::<Utc>::from_timestamp(ts, 0)
}

fn open_status_label(
    status: &squads_multisig_program::state::ProposalStatus,
) -> Option<&'static str> {
    use squads_multisig_program::state::ProposalStatus;
    match status {
        ProposalStatus::Draft { .. } => Some("draft"),
        ProposalStatus::Active { .. } => Some("active"),
        ProposalStatus::Approved { .. } => Some("approved"),
        // `Executing` is deprecated upstream and Executed/Rejected/
        // Cancelled are finalized — none are open.
        _ => None,
    }
}

/// Fetch a v4 proposal and its associated vault transaction, decode the
/// transaction message, and return a structured summary suitable for
/// signers reviewing what a proposal will do before approving it.
///
/// `multisig_or_vault` accepts whatever `super::resolve_to_multisig`
/// accepts: a v4 Multisig PDA, a vault PDA (resolved through the cache
/// and fallback scan), or any v4 transaction-bearing account
/// (Proposal / VaultTransaction / ConfigTransaction / Batch — multisig
/// is read from the body).
pub async fn get_proposal_info<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig_or_vault: &Pubkey,
    transaction_index: u64,
) -> Result<ProposalInfo, Error> {
    use squads_multisig_program::state::{Multisig, Proposal};

    let multisig_addr = super::resolve_to_multisig(client, multisig_or_vault).await?;
    let proposal_addr = proposal_pda(&multisig_addr, transaction_index);
    let transaction_addr = vault_transaction_pda(&multisig_addr, transaction_index);

    let rpc = client.as_ref();
    // The multisig is needed for threshold + stale_transaction_index.
    let multisig: Multisig = fetch_account(
        rpc,
        multisig_addr.as_pubkey(),
        &MULTISIG_DISCRIMINATOR,
        "v4 Multisig",
    )
    .await?;
    let proposal: Proposal =
        fetch_account(rpc, &proposal_addr, &PROPOSAL_DISCRIMINATOR, "v4 Proposal").await?;

    // Transaction PDAs are shared across kinds (VaultTransaction,
    // ConfigTransaction, Batch all use the same `["multisig", ms,
    // "transaction", index]` seeds). Fetch the account once and dispatch
    // on its 8-byte discriminator.
    let tx_account = rpc.get_account(&transaction_addr).await?;
    if tx_account.owner != PROGRAM_ID {
        return Err(
            DecodeError::wrong_owner(&transaction_addr, "Squads v4", &tx_account.owner).into(),
        );
    }
    let disc = super::read_discriminator(&tx_account.data)
        .ok_or_else(|| DecodeError::wrong_discriminator(&transaction_addr, "v4 transaction"))?;
    let body = &tx_account.data[8..];

    let ctx = DecodeCtx {
        multisig,
        multisig_addr,
        transaction_index,
        proposal,
        proposal_addr,
        transaction_addr,
        body,
    };
    match disc {
        VAULT_TRANSACTION_DISCRIMINATOR => decode_vault_transaction(client, ctx).await,
        CONFIG_TRANSACTION_DISCRIMINATOR => decode_config_transaction(ctx),
        BATCH_DISCRIMINATOR => decode_batch(client, ctx).await,
        _ => Err(DecodeError::wrong_discriminator(
            &transaction_addr,
            "v4 VaultTransaction / ConfigTransaction / Batch",
        )
        .into()),
    }
}

/// Shared context for the per-kind decoders: everything fetched/computed
/// before we know which transaction type we're looking at.
struct DecodeCtx<'a> {
    multisig: squads_multisig_program::state::Multisig,
    multisig_addr: MultisigKey,
    transaction_index: u64,
    proposal: squads_multisig_program::state::Proposal,
    proposal_addr: Pubkey,
    transaction_addr: Pubkey,
    body: &'a [u8],
}

async fn decode_vault_transaction<C: AsRef<SolanaRpcClient>>(
    client: &C,
    ctx: DecodeCtx<'_>,
) -> Result<ProposalInfo, Error> {
    let DecodeCtx {
        multisig,
        multisig_addr,
        transaction_index,
        proposal,
        proposal_addr,
        transaction_addr: vault_tx_addr,
        body,
    } = ctx;
    let vault_tx = squads_multisig_program::state::VaultTransaction::deserialize(&mut &body[..])
        .map_err(|e| DecodeError::deserialize(&vault_tx_addr, "v4 VaultTransaction", e))?;

    let vault = vault_pda(&multisig_addr, vault_tx.vault_index);
    let resolved_keys = resolve_account_keys(client, &vault_tx.message).await?;
    let instructions: Vec<InstructionInfo> = vault_tx
        .message
        .instructions
        .iter()
        .map(|ix| compile_instruction_info(ix, &resolved_keys))
        .collect::<Result<_, _>>()?;
    let address_lookup_tables =
        extract_lookup_tables(vault_tx.message.address_table_lookups.iter());
    let summary = build_summary(&multisig, transaction_index, &proposal, &instructions);

    Ok(ProposalInfo::VaultTransaction(VaultTransactionInfo {
        summary,
        multisig: multisig_addr,
        transaction_index,
        proposal: proposal_addr,
        vault_transaction: vault_tx_addr,
        status: convert_proposal_status(&proposal.status)?,
        votes: votes_from(&proposal),
        vault_index: vault_tx.vault_index,
        vault,
        creator: vault_tx.creator,
        ephemeral_signers: SquadsError::try_u8(
            vault_tx.ephemeral_signer_bumps.len(),
            MessageField::EphemeralSignerIndex,
        )?,
        instructions,
        address_lookup_tables,
    }))
}

fn decode_config_transaction(ctx: DecodeCtx<'_>) -> Result<ProposalInfo, Error> {
    let DecodeCtx {
        multisig,
        multisig_addr,
        transaction_index,
        proposal,
        proposal_addr,
        transaction_addr: config_tx_addr,
        body,
    } = ctx;
    let config_tx = squads_multisig_program::state::ConfigTransaction::deserialize(&mut &body[..])
        .map_err(|e| DecodeError::deserialize(&config_tx_addr, "v4 ConfigTransaction", e))?;

    let actions: Vec<ConfigActionInfo> = config_tx
        .actions
        .iter()
        .map(convert_config_action)
        .collect();
    let summary = build_summary(&multisig, transaction_index, &proposal, &[]);

    Ok(ProposalInfo::ConfigTransaction(ConfigTransactionInfo {
        summary,
        multisig: multisig_addr,
        transaction_index,
        proposal: proposal_addr,
        config_transaction: config_tx_addr,
        status: convert_proposal_status(&proposal.status)?,
        votes: votes_from(&proposal),
        creator: config_tx.creator,
        actions,
    }))
}

async fn decode_batch<C: AsRef<SolanaRpcClient>>(
    client: &C,
    ctx: DecodeCtx<'_>,
) -> Result<ProposalInfo, Error> {
    let DecodeCtx {
        multisig,
        multisig_addr,
        transaction_index,
        proposal,
        proposal_addr,
        transaction_addr: batch_addr,
        body,
    } = ctx;
    let batch = squads_multisig_program::state::Batch::deserialize(&mut &body[..])
        .map_err(|e| DecodeError::deserialize(&batch_addr, "v4 Batch", e))?;

    let vault = vault_pda(&multisig_addr, batch.vault_index);
    let sub_transactions =
        fetch_batch_sub_transactions(client, &multisig_addr, transaction_index, batch.size).await?;
    let all_instructions: Vec<InstructionInfo> = sub_transactions
        .iter()
        .flat_map(|s| s.instructions.iter().cloned())
        .collect();

    let summary = build_summary(&multisig, transaction_index, &proposal, &all_instructions);

    Ok(ProposalInfo::Batch(BatchInfo {
        summary,
        multisig: multisig_addr,
        transaction_index,
        proposal: proposal_addr,
        batch: batch_addr,
        status: convert_proposal_status(&proposal.status)?,
        votes: votes_from(&proposal),
        vault_index: batch.vault_index,
        vault,
        creator: batch.creator,
        size: batch.size,
        executed_transactions: batch.executed_transaction_index,
        sub_transactions,
    }))
}

/// Fetch and decode all sub-transactions for a Batch in parallel. Each
/// sub-tx's instructions get the same LUT-resolved account treatment as
/// a top-level VaultTransaction.
async fn fetch_batch_sub_transactions<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig_addr: &MultisigKey,
    batch_index: u64,
    size: u32,
) -> Result<Vec<BatchSubTransaction>, Error> {
    if size == 0 {
        return Ok(Vec::new());
    }
    let rpc = client.as_ref();
    let entries: Vec<(u32, Pubkey)> = (1..=size)
        .map(|i| (i, batch_transaction_pda(multisig_addr, batch_index, i)))
        .collect();

    futures::stream::iter(entries)
        .map(|(idx, addr)| async move {
            let account = rpc.get_account(&addr).await?;
            if account.owner != PROGRAM_ID
                || account.data.len() < 8
                || account.data[..8] != VAULT_BATCH_TRANSACTION_DISCRIMINATOR
            {
                return Err(
                    DecodeError::wrong_discriminator(&addr, "v4 VaultBatchTransaction").into(),
                );
            }
            let sub = squads_multisig_program::state::VaultBatchTransaction::deserialize(
                &mut &account.data[8..],
            )
            .map_err(|e| DecodeError::deserialize(&addr, "v4 VaultBatchTransaction", e))?;
            let resolved_keys = resolve_account_keys(client, &sub.message).await?;
            let instructions: Vec<InstructionInfo> = sub
                .message
                .instructions
                .iter()
                .map(|ix| compile_instruction_info(ix, &resolved_keys))
                .collect::<Result<_, _>>()?;
            let address_lookup_tables =
                extract_lookup_tables(sub.message.address_table_lookups.iter());
            Ok::<BatchSubTransaction, Error>(BatchSubTransaction {
                batch_index: idx,
                address: addr,
                ephemeral_signers: SquadsError::try_u8(
                    sub.ephemeral_signer_bumps.len(),
                    MessageField::EphemeralSignerIndex,
                )?,
                instructions,
                address_lookup_tables,
            })
        })
        .buffered(5)
        .try_collect()
        .await
}

fn convert_config_action(
    action: &squads_multisig_program::state::ConfigAction,
) -> ConfigActionInfo {
    use squads_multisig_program::state::{ConfigAction, Period};
    let period = |p: Period| match p {
        Period::OneTime => ConfigSpendingPeriod::OneTime,
        Period::Day => ConfigSpendingPeriod::Day,
        Period::Week => ConfigSpendingPeriod::Week,
        Period::Month => ConfigSpendingPeriod::Month,
    };
    match action {
        ConfigAction::AddMember { new_member } => ConfigActionInfo::AddMember {
            new_member: new_member.key,
            permissions: MemberPermissions::from_mask(new_member.permissions.mask),
        },
        ConfigAction::RemoveMember { old_member } => ConfigActionInfo::RemoveMember {
            old_member: *old_member,
        },
        ConfigAction::ChangeThreshold { new_threshold } => ConfigActionInfo::ChangeThreshold {
            new_threshold: *new_threshold,
        },
        ConfigAction::SetTimeLock { new_time_lock } => ConfigActionInfo::SetTimeLock {
            new_time_lock: *new_time_lock,
        },
        ConfigAction::AddSpendingLimit {
            create_key,
            vault_index,
            mint,
            amount,
            period: p,
            members,
            destinations,
        } => ConfigActionInfo::AddSpendingLimit {
            create_key: *create_key,
            vault_index: *vault_index,
            mint: *mint,
            amount: *amount,
            period: period(*p),
            members: members.iter().map(ToString::to_string).collect(),
            destinations: destinations.iter().map(ToString::to_string).collect(),
        },
        ConfigAction::RemoveSpendingLimit { spending_limit } => {
            ConfigActionInfo::RemoveSpendingLimit {
                spending_limit: *spending_limit,
            }
        }
        ConfigAction::SetRentCollector { new_rent_collector } => {
            ConfigActionInfo::SetRentCollector {
                new_rent_collector: *new_rent_collector,
            }
        }
        // upstream is `#[non_exhaustive]`
        _ => ConfigActionInfo::Unknown,
    }
}

/// Fetch a v4 account, verify its owner and 8-byte discriminator, and Borsh-
/// decode the body. The upstream Anchor types (Multisig / Proposal /
/// VaultTransaction) are decorated with `#[account]` against anchor-lang
/// 0.32 — a different trait surface than helium-lib's 0.31 fork — so we
/// validate manually rather than going through `AccountDeserialize`.
async fn fetch_account<T: AnchorDeserialize>(
    rpc: &SolanaRpcClient,
    address: &Pubkey,
    expected_disc: &[u8; 8],
    type_name: &'static str,
) -> Result<T, Error> {
    let account = rpc.get_account(address).await?;
    decode_account(
        address,
        &account.owner,
        &account.data,
        expected_disc,
        type_name,
    )
}

/// Validate-and-decode helper used by both fetch paths and the unified
/// `decode_multisig` entry. Exists so error sites have a single
/// authoritative source instead of repeating wrong-owner / wrong-disc /
/// borsh-failure strings at every call.
fn decode_account<T: AnchorDeserialize>(
    address: &Pubkey,
    owner: &Pubkey,
    data: &[u8],
    expected_disc: &[u8; 8],
    type_name: &'static str,
) -> Result<T, Error> {
    if *owner != PROGRAM_ID {
        return Err(DecodeError::wrong_owner(address, "Squads v4", owner).into());
    }
    if data.len() < 8 || &data[..8] != expected_disc {
        return Err(DecodeError::wrong_discriminator(address, type_name).into());
    }
    T::deserialize(&mut &data[8..])
        .map_err(|e| DecodeError::deserialize(address, type_name, e).into())
}

/// Vote tally lifted off the on-chain `Proposal`. Used by every decode
/// path that surfaces a `ProposalVotes` so the field set stays
/// consistent.
fn votes_from(proposal: &squads_multisig_program::state::Proposal) -> ProposalVotes {
    ProposalVotes {
        approved: proposal.approved.len(),
        rejected: proposal.rejected.len(),
        cancelled: proposal.cancelled.len(),
    }
}

/// Convert Squads' on-chain `MessageAddressTableLookup` entries into
/// our reviewer-facing `AddressLookupInfo` rows. Used by both the
/// top-level VaultTransaction decoder and the Batch sub-transaction
/// decoder so the surfaced shape stays in sync.
fn extract_lookup_tables<'a, I>(lookups: I) -> Vec<AddressLookupInfo>
where
    I: IntoIterator<Item = &'a squads_multisig_program::state::MultisigMessageAddressTableLookup>,
{
    lookups
        .into_iter()
        .map(|lut| AddressLookupInfo {
            address: lut.account_key,
            writable_indexes: lut.writable_indexes.clone(),
            readonly_indexes: lut.readonly_indexes.clone(),
        })
        .collect()
}

/// Compute the at-a-glance signals from already-fetched data — no extra
/// RPC calls. Order matters for `programs`: we keep the order of first
/// appearance so the summary mirrors the instruction list.
fn build_summary(
    multisig: &squads_multisig_program::state::Multisig,
    transaction_index: u64,
    proposal: &squads_multisig_program::state::Proposal,
    instructions: &[InstructionInfo],
) -> ProposalSummary {
    let approved = proposal.approved.len();
    let threshold = multisig.threshold;
    let approvals = format!("{approved}/{threshold}");
    let stale = transaction_index <= multisig.stale_transaction_index;
    super::build_summary(approvals, stale, instructions)
}

pub(super) fn decode_multisig(
    address: MultisigKey,
    data: &[u8],
    resolved_from_vault: Option<VaultKey>,
) -> Result<MultisigInfo, Error> {
    let multisig = decode_account::<squads_multisig_program::state::Multisig>(
        address.as_pubkey(),
        &PROGRAM_ID, // already validated by the caller, but recheck for safety
        data,
        &MULTISIG_DISCRIMINATOR,
        "v4 Multisig",
    )?;
    let members = multisig
        .members
        .iter()
        .map(|m| MemberInfo {
            key: m.key,
            permissions: MemberPermissions::from_mask(m.permissions.mask),
        })
        .collect();
    Ok(MultisigInfo {
        address,
        version: Version::V4,
        threshold: multisig.threshold,
        transaction_index: multisig.transaction_index,
        members,
        resolved_from_vault,
    })
}

/// Cheap check for "this account is a v4 Multisig" used during vault
/// resolution scans.
pub(super) fn is_multisig_account(owner: &Pubkey, data: &[u8]) -> bool {
    *owner == PROGRAM_ID && data.len() >= 8 && data[..8] == MULTISIG_DISCRIMINATOR
}

/// Self-identify a v4 account that already passed the owner check. Returns
/// `Ok(Some((multisig, index)))` for any of the four transaction-bearing
/// kinds (Proposal / VaultTransaction / ConfigTransaction / Batch);
/// `Ok(None)` if the account is a Multisig (caller must supply the
/// index); `Err` if the discriminator isn't one we recognize.
pub(super) fn extract_target(
    address: &Pubkey,
    data: &[u8],
) -> Result<Option<(Pubkey, u64)>, Error> {
    use squads_multisig_program::state::{Batch, ConfigTransaction, Proposal, VaultTransaction};
    let disc = super::read_discriminator(data)
        .ok_or_else(|| DecodeError::wrong_discriminator(address, "v4 account"))?;
    let body = &data[8..];
    Ok(Some(match disc {
        MULTISIG_DISCRIMINATOR => return Ok(None),
        PROPOSAL_DISCRIMINATOR => {
            let p = Proposal::deserialize(&mut &body[..])
                .map_err(|e| DecodeError::deserialize(address, "v4 Proposal", e))?;
            (p.multisig, p.transaction_index)
        }
        VAULT_TRANSACTION_DISCRIMINATOR => {
            let v = VaultTransaction::deserialize(&mut &body[..])
                .map_err(|e| DecodeError::deserialize(address, "v4 VaultTransaction", e))?;
            (v.multisig, v.index)
        }
        CONFIG_TRANSACTION_DISCRIMINATOR => {
            let c = ConfigTransaction::deserialize(&mut &body[..])
                .map_err(|e| DecodeError::deserialize(address, "v4 ConfigTransaction", e))?;
            (c.multisig, c.index)
        }
        BATCH_DISCRIMINATOR => {
            let b = Batch::deserialize(&mut &body[..])
                .map_err(|e| DecodeError::deserialize(address, "v4 Batch", e))?;
            (b.multisig, b.index)
        }
        _ => {
            return Err(DecodeError::wrong_discriminator(address, "v4 account").into());
        }
    }))
}

fn convert_proposal_status(
    status: &squads_multisig_program::state::ProposalStatus,
) -> Result<ProposalStatusInfo, Error> {
    use squads_multisig_program::state::ProposalStatus;
    // Squads timestamps are seconds since epoch; `from_timestamp`
    // returns `None` only for values outside chrono's representable
    // range (year ±262_144), which a healthy proposal never produces.
    // Surface the bad value rather than silently rendering 1970 — a
    // reviewer who sees a malformed timestamp wants to know the
    // proposal account is corrupt.
    let to_dt = |ts: i64| {
        DateTime::<Utc>::from_timestamp(ts, 0).ok_or(SquadsError::invalid_status_timestamp(ts))
    };
    #[allow(deprecated)]
    Ok(match status {
        ProposalStatus::Draft { timestamp } => ProposalStatusInfo::Draft {
            timestamp: to_dt(*timestamp)?,
        },
        ProposalStatus::Active { timestamp } => ProposalStatusInfo::Active {
            timestamp: to_dt(*timestamp)?,
        },
        ProposalStatus::Rejected { timestamp } => ProposalStatusInfo::Rejected {
            timestamp: to_dt(*timestamp)?,
        },
        ProposalStatus::Approved { timestamp } => ProposalStatusInfo::Approved {
            timestamp: to_dt(*timestamp)?,
        },
        ProposalStatus::Executed { timestamp } => ProposalStatusInfo::Executed {
            timestamp: to_dt(*timestamp)?,
        },
        ProposalStatus::Cancelled { timestamp } => ProposalStatusInfo::Cancelled {
            timestamp: to_dt(*timestamp)?,
        },
        ProposalStatus::Executing => ProposalStatusInfo::Executing,
        // Upstream `ProposalStatus` is `#[non_exhaustive]`; future variants
        // render as the deprecated transient `Executing` until we update.
        _ => ProposalStatusInfo::Executing,
    })
}

/// One entry in the resolved account list — tracks pubkey plus its
/// writable/signer status so we can render instruction account refs
/// uniformly whether they came from static keys or were materialized
/// out of an address lookup table.
struct ResolvedKey {
    pubkey: Pubkey,
    writable: bool,
    signer: bool,
}

/// Build the full account list a v4 instruction can address: static keys
/// in their original positions, then for each LUT in
/// `address_table_lookups` order, that LUT's writable-resolved section
/// followed by its readonly-resolved section. Matches the layout
/// `vault_transaction_execute` validates against in
/// `ExecutableTransactionMessage::new_validated` (per-LUT
/// writable/readonly), which is what `MultisigCompiledInstruction.
/// account_indexes` reference.
async fn resolve_account_keys<C: AsRef<SolanaRpcClient>>(
    client: &C,
    msg: &squads_multisig_program::state::VaultTransactionMessage,
) -> Result<Vec<ResolvedKey>, Error> {
    let mut keys: Vec<ResolvedKey> = msg
        .account_keys
        .iter()
        .enumerate()
        .map(|(i, key)| ResolvedKey {
            pubkey: *key,
            writable: msg.is_static_writable_index(i),
            signer: msg.is_signer_index(i),
        })
        .collect();

    if msg.address_table_lookups.is_empty() {
        return Ok(keys);
    }

    // Bulk-fetch the LUT accounts via the same raw JSON-RPC path used by
    // vault resolution — solana-client's `get_multiple_accounts` trips
    // on the rent_epoch=u64::MAX sentinel.
    let lut_addrs: Vec<Pubkey> = msg
        .address_table_lookups
        .iter()
        .map(|lut| lut.account_key)
        .collect();
    let rpc_url = client.as_ref().url();
    let lut_accounts = super::raw_get_multiple_accounts(&rpc_url, &lut_addrs).await?;

    for (lut, account) in msg.address_table_lookups.iter().zip(lut_accounts) {
        // Inspect can't render a faithful proposal view if a referenced
        // LUT is missing — the on-chain resolution would still produce
        // accounts, but our local resolution would silently drop them
        // and `compile_instruction_info` would render an instruction
        // with fewer accounts than it actually has. Match the execute
        // path's `account_not_found` bail.
        let Some(raw) = account else {
            return Err(Error::account_not_found());
        };
        let table =
            solana_sdk::address_lookup_table::state::AddressLookupTable::deserialize(&raw.data)
                .map_err(|e| DecodeError::deserialize(&lut.account_key, "LUT", e))?;
        // Same out-of-range bail as the execute path — silent skip
        // would shift later indices and break alignment with
        // compile_instruction_info.
        for key in resolve_lut_indexes(&table, &lut.account_key, &lut.writable_indexes)? {
            keys.push(ResolvedKey {
                pubkey: key,
                writable: true,
                signer: false,
            });
        }
        for key in resolve_lut_indexes(&table, &lut.account_key, &lut.readonly_indexes)? {
            keys.push(ResolvedKey {
                pubkey: key,
                writable: false,
                signer: false,
            });
        }
    }
    Ok(keys)
}

/// Resolve a slice of LUT indexes against the resolved table, bailing
/// loudly on any out-of-range index instead of silently dropping the
/// entry. Squads' on-chain handler reads the same LUT and won't tolerate
/// a missing slot — silently skipping would produce a misaligned account
/// list and ship a tx the validator either rejects or mis-applies.
fn resolve_lut_indexes(
    table: &solana_sdk::address_lookup_table::state::AddressLookupTable<'_>,
    table_addr: &Pubkey,
    indexes: &[u8],
) -> Result<Vec<Pubkey>, Error> {
    indexes
        .iter()
        .map(|&idx| {
            table
                .addresses
                .get(usize::from(idx))
                .copied()
                .ok_or_else(|| {
                    SquadsError::lut_index_out_of_range(*table_addr, idx, table.addresses.len())
                        .into()
                })
        })
        .collect()
}

fn compile_instruction_info(
    ix: &squads_multisig_program::state::MultisigCompiledInstruction,
    keys: &[ResolvedKey],
) -> Result<InstructionInfo, Error> {
    let program_idx = usize::from(ix.program_id_index);
    let program_id = keys.get(program_idx).map(|k| k.pubkey).ok_or_else(|| {
        SquadsError::instruction_index_out_of_range(
            CompiledInstructionField::ProgramIdIndex,
            program_idx,
            keys.len(),
        )
    })?;
    let accounts: Vec<InstructionAccountRef> = ix
        .account_indexes
        .iter()
        .map(|idx| {
            let i = usize::from(*idx);
            let key = keys.get(i).ok_or_else(|| {
                SquadsError::instruction_index_out_of_range(
                    CompiledInstructionField::AccountIndex,
                    i,
                    keys.len(),
                )
            })?;
            Ok::<_, Error>(InstructionAccountRef {
                pubkey: key.pubkey,
                writable: key.writable,
                signer: key.signer,
            })
        })
        .collect::<Result<_, _>>()?;
    let program = KnownProgram::from_pubkey(&program_id);
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
    Ok(InstructionInfo {
        program_id,
        program,
        method,
        args,
        accounts,
        data_len: ix.data.len(),
        discriminator,
        data_b58: bs58::encode(&ix.data).into_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MemberPermissions must round-trip through its bitmask and hold the
    /// bit-position contract with Squads' upstream `Permission` enum
    /// (`Initiate=1<<0`, `Vote=1<<1`, `Execute=1<<2`).
    #[test]
    fn member_permissions_mask_round_trip() {
        for propose in [false, true] {
            for vote in [false, true] {
                for execute in [false, true] {
                    let p = MemberPermissions {
                        propose,
                        vote,
                        execute,
                    };
                    assert_eq!(MemberPermissions::from_mask(p.to_mask()), p);
                }
            }
        }
        assert_eq!(MemberPermissions::ALL.to_mask(), 0b111);
        assert_eq!(MemberPermissions::default().to_mask(), 0);
    }

    /// `resolve_lut_indexes` is the load-bearing helper for the LUT
    /// out-of-range check: a silently-dropped index would shift later
    /// resolved entries up by one slot and misalign `remaining_accounts`.
    /// Pin both arms — in-range indexes resolve to their addresses, and any
    /// out-of-range index bails with a typed `LutIndexOutOfRange`.
    #[test]
    fn resolve_lut_indexes_in_range_and_overflow() {
        use crate::squads::error::SquadsEncodingError;
        use solana_sdk::address_lookup_table::state::{AddressLookupTable, LookupTableMeta};
        use std::borrow::Cow;

        let addrs = [
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        ];
        let table = AddressLookupTable {
            meta: LookupTableMeta::default(),
            addresses: Cow::Borrowed(&addrs),
        };
        let table_addr = Pubkey::new_unique();

        let resolved =
            resolve_lut_indexes(&table, &table_addr, &[2u8, 0u8]).expect("in-range resolves");
        assert_eq!(resolved, vec![addrs[2], addrs[0]]);

        assert_eq!(
            resolve_lut_indexes(&table, &table_addr, &[]).expect("empty"),
            Vec::<Pubkey>::new(),
        );

        let err = resolve_lut_indexes(&table, &table_addr, &[0u8, 5u8]).unwrap_err();
        match err {
            Error::Squads(SquadsError::Encoding(SquadsEncodingError::LutIndexOutOfRange {
                table,
                index,
                size,
            })) => {
                assert_eq!(table, table_addr);
                assert_eq!(index, 5);
                assert_eq!(size, addrs.len());
            }
            other => panic!("expected LutIndexOutOfRange, got {other:?}"),
        }
    }
}
