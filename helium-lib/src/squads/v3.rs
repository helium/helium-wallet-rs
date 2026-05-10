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
    error::SquadsError, InstructionAccountRef, InstructionInfo, MemberInfo, MemberPermissions,
    MultisigInfo, MultisigKey, ProposalSummary, ProposalVotes, VaultKey, Version,
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

/// Build a v3 `approve_transaction` instruction. The wallet must hold
/// the keypair for `member`, and the member must be in the multisig.
pub fn approve_transaction_ix(
    multisig: MultisigKey,
    transaction_index: u32,
    member: Pubkey,
) -> solana_sdk::instruction::Instruction {
    use anchor_lang::{InstructionData, ToAccountMetas};
    let transaction = transaction_pda(&multisig, transaction_index);
    solana_sdk::instruction::Instruction {
        program_id: PROGRAM_ID,
        accounts: squads_mpl::client::accounts::ApproveTransaction {
            multisig: multisig.into_pubkey(),
            transaction,
            member,
        }
        .to_account_metas(None),
        data: squads_mpl::client::args::ApproveTransaction {}.data(),
    }
}

/// Build a v3 `reject_transaction` instruction.
pub fn reject_transaction_ix(
    multisig: MultisigKey,
    transaction_index: u32,
    member: Pubkey,
) -> solana_sdk::instruction::Instruction {
    use anchor_lang::{InstructionData, ToAccountMetas};
    let transaction = transaction_pda(&multisig, transaction_index);
    solana_sdk::instruction::Instruction {
        program_id: PROGRAM_ID,
        accounts: squads_mpl::client::accounts::RejectTransaction {
            multisig: multisig.into_pubkey(),
            transaction,
            member,
        }
        .to_account_metas(None),
        data: squads_mpl::client::args::RejectTransaction {}.data(),
    }
}

/// Build a v3 `cancel_transaction` instruction. v3's cancel includes
/// `system_program` in the accounts list (unlike approve/reject) because
/// it may close the proposal account.
pub fn cancel_transaction_ix(
    multisig: MultisigKey,
    transaction_index: u32,
    member: Pubkey,
) -> solana_sdk::instruction::Instruction {
    use anchor_lang::{InstructionData, ToAccountMetas};
    let transaction = transaction_pda(&multisig, transaction_index);
    solana_sdk::instruction::Instruction {
        program_id: PROGRAM_ID,
        accounts: squads_mpl::client::accounts::CancelTransaction {
            multisig: multisig.into_pubkey(),
            transaction,
            member,
            system_program: solana_sdk::system_program::ID,
        }
        .to_account_metas(None),
        data: squads_mpl::client::args::CancelTransaction {}.data(),
    }
}

/// Build a v3 `execute_transaction` instruction. v3 execute differs from
/// v4 by passing accounts via `account_list: Vec<u8>` — each byte is an
/// index into `remaining_accounts`, and the program walks them in
/// `[msix_pda, program_id, ix_account_1, ix_account_2, ...]` blocks per
/// inner instruction.
///
/// Builds a deduplicated `remaining_accounts` list with the AccountMeta
/// flags Squads expects (writable from `MsAccountMeta`, signer always
/// false at the outer level — the v3 authority signs via invoke_signed
/// inside the program), then emits the `account_list` indexes in the
/// per-instruction order the program iterates.
pub async fn execute_transaction_ix<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: MultisigKey,
    transaction_index: u32,
    member: Pubkey,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    use anchor_lang::{InstructionData, ToAccountMetas};

    let transaction_addr = transaction_pda(&multisig, transaction_index);
    let rpc = client.as_ref();
    let transaction = fetch_account::<squads_mpl::accounts::MsTransaction>(
        rpc,
        &transaction_addr,
        "v3 MsTransaction",
    )
    .await?;
    let authority = authority_pda(&multisig, transaction.authority_index);

    // Fetch every MsInstruction PDA (1..=instruction_index, 1-based).
    let ix_addrs: Vec<Pubkey> = (1..=transaction.instruction_index)
        .map(|i| instruction_pda(&transaction_addr, i))
        .collect();
    let ms_ixs: Vec<squads_mpl::accounts::MsInstruction> =
        futures::stream::iter(ix_addrs.iter().copied())
            .map(|addr| async move {
                fetch_account::<squads_mpl::accounts::MsInstruction>(rpc, &addr, "v3 MsInstruction")
                    .await
            })
            .buffered(10)
            .try_collect()
            .await?;

    let (remaining, account_list) =
        build_execute_accounts(authority.as_pubkey(), &ix_addrs, &ms_ixs)?;

    let mut accounts = squads_mpl::client::accounts::ExecuteTransaction {
        multisig: multisig.into_pubkey(),
        transaction: transaction_addr,
        member,
    }
    .to_account_metas(None);
    accounts.extend(remaining);

    Ok(solana_sdk::instruction::Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data: squads_mpl::client::args::ExecuteTransaction { account_list }.data(),
    })
}

/// Pure builder used by `execute_transaction_ix`. Walks the per-
/// instruction `[msix_pda, program_id, key1, key2, ...]` blocks the
/// program iterates, deduplicates by pubkey, promotes writable when a
/// later reference is writable, and strips the `is_signer` flag for
/// the multisig authority (the program signs that PDA via
/// `invoke_signed`). Returns the deduped `remaining_accounts` and the
/// flat `account_list` of indexes the program walks.
fn build_execute_accounts(
    authority: &Pubkey,
    ix_addrs: &[Pubkey],
    ms_ixs: &[squads_mpl::accounts::MsInstruction],
) -> Result<(Vec<solana_sdk::instruction::AccountMeta>, Vec<u8>), Error> {
    use solana_sdk::instruction::AccountMeta;
    use std::collections::HashMap;

    let mut unique: Vec<AccountMeta> = Vec::new();
    let mut index_of: HashMap<Pubkey, u8> = HashMap::new();
    let mut account_list: Vec<u8> = Vec::new();

    let mut push_account = |meta: AccountMeta| -> Result<(), Error> {
        match index_of.get(&meta.pubkey) {
            Some(&idx) => {
                let entry = &mut unique[idx as usize];
                entry.is_writable = entry.is_writable || meta.is_writable;
                account_list.push(idx);
            }
            None => {
                let idx_usize = unique.len();
                let idx = u8::try_from(idx_usize)
                    .map_err(|_| SquadsError::remaining_accounts_overflow(idx_usize))?;
                index_of.insert(meta.pubkey, idx);
                unique.push(meta);
                account_list.push(idx);
            }
        }
        Ok(())
    };

    for (i, ms_ix) in ms_ixs.iter().enumerate() {
        push_account(AccountMeta::new_readonly(ix_addrs[i], false))?;
        push_account(AccountMeta::new_readonly(ms_ix.program_id, false))?;
        for key in &ms_ix.keys {
            push_account(AccountMeta {
                pubkey: key.pubkey,
                is_writable: key.is_writable,
                is_signer: key.is_signer && key.pubkey != *authority,
            })?;
        }
    }

    Ok((unique, account_list))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::Discriminator;

    /// Each vote ix's data starts with the program's discriminator for
    /// that method and the accounts come out in the documented order.
    #[test]
    fn approve_transaction_shape() {
        let multisig = MultisigKey::from_pubkey(Pubkey::new_unique());
        let member = Pubkey::new_unique();
        let ix = approve_transaction_ix(multisig, 7, member);

        assert_eq!(ix.program_id, PROGRAM_ID);
        assert_eq!(
            &ix.data[..8],
            squads_mpl::client::args::ApproveTransaction::DISCRIMINATOR
        );
        assert_eq!(ix.accounts.len(), 3);
        assert_eq!(ix.accounts[0].pubkey, multisig.into_pubkey());
        assert!(!ix.accounts[0].is_signer);
        assert_eq!(ix.accounts[1].pubkey, transaction_pda(&multisig, 7));
        assert!(ix.accounts[1].is_writable);
        assert_eq!(ix.accounts[2].pubkey, member);
        assert!(ix.accounts[2].is_signer);
    }

    #[test]
    fn cancel_includes_system_program() {
        let ix = cancel_transaction_ix(
            MultisigKey::from_pubkey(Pubkey::new_unique()),
            1,
            Pubkey::new_unique(),
        );
        assert_eq!(ix.accounts.len(), 4);
        assert_eq!(ix.accounts[3].pubkey, solana_sdk::system_program::ID);
    }

    #[test]
    fn vote_discriminators_distinct() {
        let approve = squads_mpl::client::args::ApproveTransaction::DISCRIMINATOR;
        let reject = squads_mpl::client::args::RejectTransaction::DISCRIMINATOR;
        let cancel = squads_mpl::client::args::CancelTransaction::DISCRIMINATOR;
        assert_ne!(approve, reject);
        assert_ne!(approve, cancel);
        assert_ne!(reject, cancel);
    }

    /// Cover the dedup + writable-promotion + signer-strip rules
    /// `build_execute_accounts` enforces. Two synthetic instructions
    /// share two accounts (the v3 authority + an arbitrary account)
    /// and one shows up readonly first then writable second — the
    /// builder must collapse to one entry, mark it writable, and
    /// strip `is_signer` from the authority.
    #[test]
    fn execute_dedup_promote_strip() {
        use squads_mpl::accounts::MsInstruction;
        use squads_mpl::types::MsAccountMeta;

        let multisig = MultisigKey::from_pubkey(Pubkey::new_unique());
        let authority = authority_pda(&multisig, 1);
        let authority_pk = authority.into_pubkey();
        let program_a = Pubkey::new_unique();
        let program_b = Pubkey::new_unique();
        let shared = Pubkey::new_unique();

        let ix_addrs = [
            instruction_pda(&Pubkey::new_unique(), 1),
            instruction_pda(&Pubkey::new_unique(), 2),
        ];

        let ms_ixs = vec![
            MsInstruction {
                program_id: program_a,
                keys: vec![
                    // Authority appears with is_signer=true; must be
                    // stripped because the program signs via invoke_signed.
                    MsAccountMeta {
                        pubkey: authority_pk,
                        is_signer: true,
                        is_writable: true,
                    },
                    // First mention of `shared` is readonly.
                    MsAccountMeta {
                        pubkey: shared,
                        is_signer: false,
                        is_writable: false,
                    },
                ],
                data: vec![],
                instruction_index: 1,
                bump: 0,
                executed: false,
            },
            MsInstruction {
                program_id: program_b,
                keys: vec![
                    // Second mention of `shared` is writable — must
                    // promote the dedup'd entry to writable.
                    MsAccountMeta {
                        pubkey: shared,
                        is_signer: false,
                        is_writable: true,
                    },
                ],
                data: vec![],
                instruction_index: 2,
                bump: 0,
                executed: false,
            },
        ];

        let (remaining, account_list) =
            build_execute_accounts(&authority_pk, &ix_addrs, &ms_ixs).expect("build");

        // 6 unique accounts: ix_pda_0, program_a, authority, shared,
        // ix_pda_1, program_b. (`shared` collapses across the two ixs;
        // `authority` is unique.)
        assert_eq!(remaining.len(), 6);
        let by_key: std::collections::HashMap<Pubkey, &solana_sdk::instruction::AccountMeta> =
            remaining.iter().map(|m| (m.pubkey, m)).collect();

        let auth_entry = by_key[&authority_pk];
        assert!(
            !auth_entry.is_signer,
            "authority must lose is_signer at the outer level",
        );
        let shared_entry = by_key[&shared];
        assert!(
            shared_entry.is_writable,
            "shared must be promoted to writable",
        );

        // account_list walks: [ix0_pda, program_a, authority, shared,
        //                      ix1_pda, program_b, shared].
        assert_eq!(account_list.len(), 7);
        // Last entry refers to `shared` — same index as the earlier mention.
        assert_eq!(account_list[3], account_list[6]);
    }
}
