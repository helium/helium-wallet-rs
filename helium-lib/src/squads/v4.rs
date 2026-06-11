//! Squads v4 (`SQDS4ep…`). Types come from the upstream
//! `squads-multisig-program` crate.

use super::{
    error::{CompiledInstructionField, MessageField, SquadsError},
    InstructionAccountRef, InstructionInfo, MemberInfo, MemberPermissions, MultisigInfo,
    MultisigKey, ProposalSummary, ProposalVotes, VaultKey, Version,
};
use crate::{
    client::SolanaRpcClient,
    error::{DecodeError, EncodeError, Error},
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

/// Anchor instruction discriminators (`sha256("global:<fn>")[..8]`) for
/// the vote/execute methods we build instructions for. Hardcoded for the
/// same reason the account discriminators are: the upstream
/// squads-multisig-program crate uses anchor-lang 0.32 while helium-lib
/// uses 0.31, so we don't share the trait machinery that would expose
/// these via codegen.
const PROPOSAL_APPROVE_IX: [u8; 8] = [144, 37, 164, 136, 188, 216, 42, 248];
const PROPOSAL_REJECT_IX: [u8; 8] = [243, 62, 134, 156, 230, 106, 246, 135];
const PROPOSAL_CANCEL_IX: [u8; 8] = [27, 42, 127, 237, 38, 163, 84, 203];

/// Build a `proposal_approve` instruction: vote yes on the proposal at
/// `(multisig, transaction_index)` as `member`. The wallet running this
/// must hold the keypair for `member`.
pub fn proposal_approve_ix(
    multisig: MultisigKey,
    transaction_index: u64,
    member: Pubkey,
    memo: Option<String>,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    proposal_vote_ix(
        multisig,
        transaction_index,
        member,
        memo,
        &PROPOSAL_APPROVE_IX,
    )
}

/// Build a `proposal_reject` instruction.
pub fn proposal_reject_ix(
    multisig: MultisigKey,
    transaction_index: u64,
    member: Pubkey,
    memo: Option<String>,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    proposal_vote_ix(
        multisig,
        transaction_index,
        member,
        memo,
        &PROPOSAL_REJECT_IX,
    )
}

/// Build a `proposal_cancel` instruction. Only valid against an
/// already-Approved proposal (reverses an approval before execution).
pub fn proposal_cancel_ix(
    multisig: MultisigKey,
    transaction_index: u64,
    member: Pubkey,
    memo: Option<String>,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    proposal_vote_ix(
        multisig,
        transaction_index,
        member,
        memo,
        &PROPOSAL_CANCEL_IX,
    )
}

const VAULT_TRANSACTION_EXECUTE_IX: [u8; 8] = [194, 8, 161, 87, 153, 164, 25, 171];
const VAULT_TRANSACTION_CREATE_IX: [u8; 8] = [48, 250, 78, 168, 208, 226, 218, 211];
const CONFIG_TRANSACTION_CREATE_IX: [u8; 8] = [155, 236, 87, 228, 137, 75, 81, 39];
const CONFIG_TRANSACTION_EXECUTE_IX: [u8; 8] = [114, 146, 244, 189, 252, 140, 36, 40];
const PROPOSAL_CREATE_IX: [u8; 8] = [220, 60, 73, 224, 30, 108, 79, 159];

/// Compile a list of `solana_sdk::Instruction` into the byte-string
/// form Squads' `vault_transaction_create` expects in
/// `VaultTransactionCreateArgs.transaction_message`. Thin wrapper
/// around `compile_transaction_message_with_luts` with an empty LUT
/// list — every account ends up inlined as a static key.
///
/// Fails with `SquadsError::MessageFieldOverflow` when the input
/// exceeds Squads' wire-format limits (u8-counted account_keys /
/// instructions / per-ix accounts / LUTs, u16-counted ix data).
pub fn compile_transaction_message(
    vault: &Pubkey,
    instructions: &[solana_sdk::instruction::Instruction],
) -> Result<Vec<u8>, Error> {
    compile_transaction_message_with_luts(vault, instructions, &[])
}

/// LUT-aware compactor. `luts` is the list of address-lookup-table
/// accounts available to the proposal — each holds the account_key (the
/// table's address) and `addresses` (the resolved pubkeys). Accounts in
/// the input instructions that match a LUT entry get encoded as a
/// lookup reference rather than a static key, which compresses the
/// proposal payload below the 1232-byte transaction-size limit when
/// many accounts are involved.
///
/// Account classification:
///   - Vault: forced to static, leads `writable_signers` at index 0.
///   - Signer of any inner ix: must be static (LUTs can't carry
///     signatures).
///   - Program IDs of all inner ixs: kept static for parity with how
///     Solana clients typically build messages and to keep
///     `MultisigCompiledInstruction.program_id_index` resolution simple.
///   - Everything else: LUT-resolvable when found in any provided LUT.
///     The first LUT containing the account wins.
pub fn compile_transaction_message_with_luts(
    vault: &Pubkey,
    instructions: &[solana_sdk::instruction::Instruction],
    luts: &[solana_sdk::address_lookup_table::AddressLookupTableAccount],
) -> Result<Vec<u8>, Error> {
    use std::collections::HashMap;

    #[derive(Clone, Copy, Default)]
    struct Flags {
        signer: bool,
        writable: bool,
    }

    let mut flags: HashMap<Pubkey, Flags> = HashMap::new();
    flags.insert(
        *vault,
        Flags {
            signer: true,
            writable: true,
        },
    );
    let mut program_ids: std::collections::HashSet<Pubkey> = std::collections::HashSet::new();
    for ix in instructions {
        program_ids.insert(ix.program_id);
        flags.entry(ix.program_id).or_default();
        for meta in &ix.accounts {
            let entry = flags.entry(meta.pubkey).or_default();
            entry.signer |= meta.is_signer;
            entry.writable |= meta.is_writable;
        }
    }

    // For each account, decide static vs LUT-resolvable. LUT-resolvable
    // tracks which LUT and which index inside it.
    struct LutRef {
        lut_idx: usize, // index into `luts`
        addr_idx: u8,   // index into that LUT's addresses
        writable: bool,
    }
    let mut lut_refs: HashMap<Pubkey, LutRef> = HashMap::new();
    if !luts.is_empty() {
        for (key, f) in flags.iter() {
            // Anything that needs to sign (including the vault) must be
            // static. Same for program IDs.
            if *key == *vault || f.signer || program_ids.contains(key) {
                continue;
            }
            for (lut_idx, lut) in luts.iter().enumerate() {
                if let Some(addr_idx) = lut.addresses.iter().position(|a| a == key) {
                    let addr_idx = match u8::try_from(addr_idx) {
                        Ok(v) => v,
                        Err(_) => continue, // LUT slot beyond u8 — skip
                    };
                    lut_refs.insert(
                        *key,
                        LutRef {
                            lut_idx,
                            addr_idx,
                            writable: f.writable,
                        },
                    );
                    break;
                }
            }
        }
    }

    // Static section: anything not in `lut_refs`. Vault leads the
    // writable_signers section. Remaining keys are sorted by pubkey
    // bytes within each (signer, writable) bucket so the same input
    // always produces the same `transaction_message` payload — Squads
    // doesn't care about ordering inside a bucket, but external tools
    // that hash proposals do.
    flags.remove(vault);
    let mut writable_signers: Vec<Pubkey> = vec![*vault];
    let mut readonly_signers: Vec<Pubkey> = Vec::new();
    let mut writable_non_signers: Vec<Pubkey> = Vec::new();
    let mut readonly_non_signers: Vec<Pubkey> = Vec::new();
    let mut sorted: Vec<(Pubkey, Flags)> = flags
        .into_iter()
        .filter(|(key, _)| !lut_refs.contains_key(key))
        .collect();
    sorted.sort_by_key(|(key, _)| key.to_bytes());
    for (key, f) in sorted {
        match (f.signer, f.writable) {
            (true, true) => writable_signers.push(key),
            (true, false) => readonly_signers.push(key),
            (false, true) => writable_non_signers.push(key),
            (false, false) => readonly_non_signers.push(key),
        }
    }

    let num_writable_signers = writable_signers.len();
    let num_signers = num_writable_signers + readonly_signers.len();
    let num_writable_non_signers = writable_non_signers.len();

    let static_keys: Vec<Pubkey> = writable_signers
        .into_iter()
        .chain(readonly_signers)
        .chain(writable_non_signers)
        .chain(readonly_non_signers)
        .collect();

    // Reject before we even allocate index slots — both the static
    // keys count itself and any later LUT-resolved index will need to
    // fit in the same u8 namespace.
    let _ = SquadsError::try_u8(static_keys.len(), MessageField::AccountKeys)?;
    let mut key_to_index: HashMap<Pubkey, u8> = static_keys
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i as u8))
        .collect();

    // Layout the LUT-resolved section per Squads' execute-validation
    // order: per LUT in `luts` order, writable entries first then
    // readonly. Build the index map alongside — accounts hit at
    // static_keys.len() + offset in the resolved section.
    struct UsedLut {
        account_key: Pubkey,
        writable_indexes: Vec<u8>,
        readonly_indexes: Vec<u8>,
    }
    let mut used_luts: Vec<UsedLut> = Vec::new();
    let mut next_idx = static_keys.len();
    // Sort lut_refs once into a deterministic ordering keyed by pubkey
    // bytes so the per-LUT writable/readonly entries land in the same
    // sequence on every run.
    let mut sorted_lut_refs: Vec<(Pubkey, &LutRef)> =
        lut_refs.iter().map(|(k, r)| (*k, r)).collect();
    sorted_lut_refs.sort_by_key(|(key, _)| key.to_bytes());
    for (lut_idx, lut) in luts.iter().enumerate() {
        let mut writable_indexes: Vec<u8> = Vec::new();
        let mut readonly_indexes: Vec<u8> = Vec::new();
        let mut writable_keys: Vec<Pubkey> = Vec::new();
        let mut readonly_keys: Vec<Pubkey> = Vec::new();
        for (key, lut_ref) in sorted_lut_refs.iter() {
            if lut_ref.lut_idx != lut_idx {
                continue;
            }
            if lut_ref.writable {
                writable_indexes.push(lut_ref.addr_idx);
                writable_keys.push(*key);
            } else {
                readonly_indexes.push(lut_ref.addr_idx);
                readonly_keys.push(*key);
            }
        }
        if writable_indexes.is_empty() && readonly_indexes.is_empty() {
            continue;
        }
        for k in &writable_keys {
            key_to_index.insert(
                *k,
                SquadsError::try_u8(next_idx, MessageField::LutResolvedIndex)?,
            );
            next_idx += 1;
        }
        for k in &readonly_keys {
            key_to_index.insert(
                *k,
                SquadsError::try_u8(next_idx, MessageField::LutResolvedIndex)?,
            );
            next_idx += 1;
        }
        used_luts.push(UsedLut {
            account_key: lut.key,
            writable_indexes,
            readonly_indexes,
        });
    }

    let mut bytes: Vec<u8> = vec![
        SquadsError::try_u8(num_signers, MessageField::NumSigners)?,
        SquadsError::try_u8(num_writable_signers, MessageField::NumWritableSigners)?,
        SquadsError::try_u8(
            num_writable_non_signers,
            MessageField::NumWritableNonSigners,
        )?,
    ];

    // SmallVec<u8, Pubkey>: u8 length + N×32 bytes.
    bytes.push(SquadsError::try_u8(
        static_keys.len(),
        MessageField::AccountKeys,
    )?);
    for k in &static_keys {
        bytes.extend_from_slice(k.as_ref());
    }

    // SmallVec<u8, CompiledInstruction>.
    bytes.push(SquadsError::try_u8(
        instructions.len(),
        MessageField::Instructions,
    )?);
    for ix in instructions {
        bytes.push(
            *key_to_index
                .get(&ix.program_id)
                .expect("program collected above"),
        );
        bytes.push(SquadsError::try_u8(
            ix.accounts.len(),
            MessageField::InstructionAccounts,
        )?);
        for meta in &ix.accounts {
            bytes.push(
                *key_to_index
                    .get(&meta.pubkey)
                    .expect("account collected above"),
            );
        }
        bytes.extend_from_slice(
            &SquadsError::try_u16(ix.data.len(), MessageField::InstructionDataLen)?.to_le_bytes(),
        );
        bytes.extend_from_slice(&ix.data);
    }

    // SmallVec<u8, MessageAddressTableLookup> = u8 length + per-LUT.
    // Each lookup: account_key (Pubkey, 32 bytes), writable_indexes
    // (SmallVec<u8, u8>), readonly_indexes (SmallVec<u8, u8>).
    bytes.push(SquadsError::try_u8(
        used_luts.len(),
        MessageField::LutCount,
    )?);
    for lut in &used_luts {
        bytes.extend_from_slice(lut.account_key.as_ref());
        bytes.push(SquadsError::try_u8(
            lut.writable_indexes.len(),
            MessageField::LutWritableIndexes,
        )?);
        bytes.extend_from_slice(&lut.writable_indexes);
        bytes.push(SquadsError::try_u8(
            lut.readonly_indexes.len(),
            MessageField::LutReadonlyIndexes,
        )?);
        bytes.extend_from_slice(&lut.readonly_indexes);
    }

    Ok(bytes)
}

/// Build a `vault_transaction_create` instruction. Allocates the
/// VaultTransaction PDA at index `multisig.transaction_index + 1`. The
/// caller is responsible for fetching the current `transaction_index`
/// from the multisig and passing the *next* one in here.
pub fn vault_transaction_create_ix(
    multisig: MultisigKey,
    next_transaction_index: u64,
    creator: Pubkey,
    vault_index: u8,
    ephemeral_signers: u8,
    transaction_message: Vec<u8>,
    memo: Option<String>,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    use anchor_lang::AnchorSerialize;
    use solana_sdk::instruction::{AccountMeta, Instruction};

    let args = squads_multisig_program::instructions::VaultTransactionCreateArgs {
        vault_index,
        ephemeral_signers,
        transaction_message,
        memo,
    };
    let mut data = VAULT_TRANSACTION_CREATE_IX.to_vec();
    args.serialize(&mut data)
        .map_err(|e| EncodeError::borsh("VaultTransactionCreateArgs", e))?;

    let transaction = vault_transaction_pda(&multisig, next_transaction_index);
    Ok(Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(multisig.into_pubkey(), false),
            AccountMeta::new(transaction, false),
            AccountMeta::new_readonly(creator, true),
            AccountMeta::new(creator, true), // rent_payer = creator
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    })
}

/// CLI-shaped view of the `ConfigAction` variants we expose. Maps
/// 1:1 onto the upstream `state::ConfigAction` for the variants the
/// wallet actually proposes (member changes, threshold changes,
/// time-lock). Spending-limit and rent-collector actions aren't
/// surfaced — they're rare and have parameter shapes the CLI would
/// awkwardly need to express.
///
/// `AddMember.permissions` uses the typed `MemberPermissions` struct
/// rather than a raw `u8` mask so the CLI side can't accidentally
/// pass undefined bits — the bit encoding lives entirely in
/// `MemberPermissions::to_mask`.
#[derive(Debug, Clone)]
pub enum ConfigActionInput {
    AddMember {
        new_member: Pubkey,
        permissions: MemberPermissions,
    },
    RemoveMember {
        old_member: Pubkey,
    },
    ChangeThreshold {
        new_threshold: u16,
    },
    SetTimeLock {
        new_time_lock: u32,
    },
}

impl From<ConfigActionInput> for squads_multisig_program::state::ConfigAction {
    fn from(value: ConfigActionInput) -> Self {
        use squads_multisig_program::state::{ConfigAction, Member, Permissions};
        match value {
            ConfigActionInput::AddMember {
                new_member,
                permissions,
            } => ConfigAction::AddMember {
                new_member: Member {
                    key: new_member,
                    permissions: Permissions {
                        mask: permissions.to_mask(),
                    },
                },
            },
            ConfigActionInput::RemoveMember { old_member } => {
                ConfigAction::RemoveMember { old_member }
            }
            ConfigActionInput::ChangeThreshold { new_threshold } => {
                ConfigAction::ChangeThreshold { new_threshold }
            }
            ConfigActionInput::SetTimeLock { new_time_lock } => {
                ConfigAction::SetTimeLock { new_time_lock }
            }
        }
    }
}

/// Build a `config_transaction_create` instruction. Allocates the
/// ConfigTransaction PDA at the same `(multisig, "transaction",
/// next_index)` seed as a vault transaction — the kind is determined
/// by the discriminator at the head of the account, not the seed.
/// Used to propose member changes, threshold changes, time-lock
/// changes, and spending-limit/rent-collector edits.
///
/// When this proposal eventually executes, Squads' on-chain handler
/// advances `multisig.stale_transaction_index` to the current
/// `transaction_index`, invalidating any pending vault transactions
/// created against the old member/threshold set. That's the protocol
/// guarantee callers rely on for security; we just produce the ix.
pub fn config_transaction_create_ix(
    multisig: MultisigKey,
    next_transaction_index: u64,
    creator: Pubkey,
    actions: Vec<squads_multisig_program::state::ConfigAction>,
    memo: Option<String>,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    use anchor_lang::AnchorSerialize;
    use solana_sdk::instruction::{AccountMeta, Instruction};

    let args = squads_multisig_program::instructions::ConfigTransactionCreateArgs { actions, memo };
    let mut data = CONFIG_TRANSACTION_CREATE_IX.to_vec();
    args.serialize(&mut data)
        .map_err(|e| EncodeError::borsh("ConfigTransactionCreateArgs", e))?;

    let transaction = vault_transaction_pda(&multisig, next_transaction_index);
    Ok(Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(multisig.into_pubkey(), false),
            AccountMeta::new(transaction, false),
            AccountMeta::new_readonly(creator, true),
            AccountMeta::new(creator, true), // rent_payer = creator
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    })
}

/// Build a `proposal_create` instruction. Pairs with
/// `vault_transaction_create_ix` to atomically create the proposal in
/// `Active` status (or `Draft` when `draft=true`).
pub fn proposal_create_ix(
    multisig: MultisigKey,
    transaction_index: u64,
    creator: Pubkey,
    draft: bool,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    use anchor_lang::AnchorSerialize;
    use solana_sdk::instruction::{AccountMeta, Instruction};

    let args = squads_multisig_program::instructions::ProposalCreateArgs {
        transaction_index,
        draft,
    };
    let mut data = PROPOSAL_CREATE_IX.to_vec();
    args.serialize(&mut data)
        .map_err(|e| EncodeError::borsh("ProposalCreateArgs", e))?;

    let proposal = proposal_pda(&multisig, transaction_index);
    Ok(Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(multisig.into_pubkey(), false),
            AccountMeta::new(proposal, false),
            AccountMeta::new_readonly(creator, true),
            AccountMeta::new(creator, true), // rent_payer = creator
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    })
}

/// One-shot helper: fetch the multisig to learn the next index, compile
/// the instructions, and return the
/// `[vault_transaction_create, proposal_create]` pair the proposer signs
/// to submit a new proposal in `Active` state.
pub async fn propose_ixs<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: MultisigKey,
    vault_index: u8,
    creator: Pubkey,
    instructions: &[solana_sdk::instruction::Instruction],
    memo: Option<String>,
) -> Result<Vec<solana_sdk::instruction::Instruction>, Error> {
    propose_ixs_with_luts(
        client,
        multisig,
        vault_index,
        creator,
        instructions,
        memo,
        &[],
    )
    .await
    .map(|(ixs, _index)| ixs)
}

/// LUT-aware variant of `propose_ixs`. `lut_addresses` is the list of
/// address-lookup-table account keys to fetch and consult while
/// compiling the message. Accounts in the input instructions that
/// match a LUT entry get encoded as a lookup reference rather than a
/// static key, keeping the proposal payload under the 1232-byte
/// transaction-size limit when many accounts are involved.
///
/// Returns the proposer ix pair plus the proposal's `transaction_index`
/// — the same value the on-chain Squads program will use for the
/// VaultTransaction / Proposal PDAs. Callers that need to surface the
/// post-submit handle (so reviewers can `squads inspect <vault>
/// --index <n>`) take the index; callers that don't drop it.
pub async fn propose_ixs_with_luts<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: MultisigKey,
    vault_index: u8,
    creator: Pubkey,
    instructions: &[solana_sdk::instruction::Instruction],
    memo: Option<String>,
    lut_addresses: &[Pubkey],
) -> Result<(Vec<solana_sdk::instruction::Instruction>, u64), Error> {
    let rpc = client.as_ref();
    let multisig_state: squads_multisig_program::state::Multisig = fetch_account(
        rpc,
        multisig.as_pubkey(),
        &MULTISIG_DISCRIMINATOR,
        "v4 Multisig",
    )
    .await?;
    let next_index = multisig_state
        .transaction_index
        .checked_add(1)
        .ok_or_else(SquadsError::transaction_index_overflow)?;

    let luts = if lut_addresses.is_empty() {
        Vec::new()
    } else {
        crate::message::get_lut_accounts(client, lut_addresses).await?
    };

    let vault = vault_pda(&multisig, vault_index);
    let message = compile_transaction_message_with_luts(vault.as_pubkey(), instructions, &luts)?;
    let create =
        vault_transaction_create_ix(multisig, next_index, creator, vault_index, 0, message, memo)?;
    let proposal = proposal_create_ix(multisig, next_index, creator, false)?;
    Ok((vec![create, proposal], next_index))
}

/// One-shot helper for member/threshold/config changes: fetch the
/// multisig to learn the next index, build the
/// `[config_transaction_create, proposal_create]` pair, and return
/// it alongside the proposal's transaction index. Mirrors
/// `propose_ixs_with_luts` for the config-change flow — config
/// transactions don't have inner instructions to LUT-resolve, so
/// this variant doesn't take an `lut_addresses` parameter.
pub async fn propose_config_change_ixs<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: MultisigKey,
    creator: Pubkey,
    actions: Vec<squads_multisig_program::state::ConfigAction>,
    memo: Option<String>,
) -> Result<(Vec<solana_sdk::instruction::Instruction>, u64), Error> {
    let rpc = client.as_ref();
    let multisig_state: squads_multisig_program::state::Multisig = fetch_account(
        rpc,
        multisig.as_pubkey(),
        &MULTISIG_DISCRIMINATOR,
        "v4 Multisig",
    )
    .await?;
    let next_index = multisig_state
        .transaction_index
        .checked_add(1)
        .ok_or_else(SquadsError::transaction_index_overflow)?;

    let create = config_transaction_create_ix(multisig, next_index, creator, actions, memo)?;
    let proposal = proposal_create_ix(multisig, next_index, creator, false)?;
    Ok((vec![create, proposal], next_index))
}

/// Build a `vault_transaction_execute` instruction. Executes a v4
/// proposal that has reached `Approved` status and (if applicable)
/// passed its time lock. The wallet running this must hold the keypair
/// for `member` and the member must have `Execute` permission on the
/// multisig.
///
/// `remaining_accounts` are assembled in the exact order Squads
/// validates against:
///   1. LUT accounts in `message.address_table_lookups` order
///   2. Static `message.account_keys` in their original message order
///   3. For each LUT: that LUT's writable-resolved section, then its
///      readonly-resolved section
///
/// Signer flags are propagated from the message except for the vault PDA
/// and ephemeral signer PDAs (those sign via `invoke_signed` inside the
/// program, not by the outer transaction).
pub async fn vault_transaction_execute_ix<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: MultisigKey,
    transaction_index: u64,
    member: Pubkey,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    let vault_tx_addr = vault_transaction_pda(&multisig, transaction_index);

    // Need the on-chain VaultTransaction to know the message + vault_index +
    // ephemeral_signer_bumps for the remaining_accounts assembly.
    let rpc = client.as_ref();
    let vault_tx: squads_multisig_program::state::VaultTransaction = fetch_account(
        rpc,
        &vault_tx_addr,
        &VAULT_TRANSACTION_DISCRIMINATOR,
        "v4 VaultTransaction",
    )
    .await?;

    // Resolve every LUT referenced by the message into
    // `(writable_resolved, readonly_resolved)` pubkey slices, in the
    // same order as `vault_tx.message.address_table_lookups`. Bails if
    // any index falls outside the table — silently dropping would
    // misalign downstream account positions.
    let lut_addrs: Vec<Pubkey> = vault_tx
        .message
        .address_table_lookups
        .iter()
        .map(|a| a.account_key)
        .collect();
    let mut lut_resolutions: Vec<(Vec<Pubkey>, Vec<Pubkey>)> = Vec::new();
    if !lut_addrs.is_empty() {
        let luts = super::raw_get_multiple_accounts(&rpc.url(), &lut_addrs).await?;
        for (lookup, account) in vault_tx.message.address_table_lookups.iter().zip(luts) {
            let Some(raw) = account else {
                return Err(Error::account_not_found());
            };
            let table =
                solana_sdk::address_lookup_table::state::AddressLookupTable::deserialize(&raw.data)
                    .map_err(|e| DecodeError::deserialize(&lookup.account_key, "LUT", e))?;
            let writable =
                resolve_lut_indexes(&table, &lookup.account_key, &lookup.writable_indexes)?;
            let readonly =
                resolve_lut_indexes(&table, &lookup.account_key, &lookup.readonly_indexes)?;
            lut_resolutions.push((writable, readonly));
        }
    }

    assemble_vault_execute_ix(
        multisig,
        transaction_index,
        member,
        &vault_tx,
        &lut_resolutions,
    )
}

/// Pure assembly of the `vault_transaction_execute` instruction given a
/// fully-fetched `VaultTransaction` and pre-resolved LUT entries.
/// Split from `vault_transaction_execute_ix` so the strict three-section
/// account ordering (LUT account_keys → static keys → per-LUT
/// writable-then-readonly), signer-strip for vault + ephemeral-signer
/// PDAs, and ephemeral-signer PDA derivation can be unit-tested
/// without RPC.
///
/// `lut_resolutions[i]` is the `(writable, readonly)` pubkey slice for
/// `vault_tx.message.address_table_lookups[i]`, in the same order.
fn assemble_vault_execute_ix(
    multisig: MultisigKey,
    transaction_index: u64,
    member: Pubkey,
    vault_tx: &squads_multisig_program::state::VaultTransaction,
    lut_resolutions: &[(Vec<Pubkey>, Vec<Pubkey>)],
) -> Result<solana_sdk::instruction::Instruction, Error> {
    use solana_sdk::instruction::{AccountMeta, Instruction};

    let proposal = proposal_pda(&multisig, transaction_index);
    let vault_tx_addr = vault_transaction_pda(&multisig, transaction_index);
    let msg = &vault_tx.message;
    let vault_addr = vault_pda(&multisig, vault_tx.vault_index);

    // Ephemeral signer PDAs are seeded `[SEED_PREFIX, vault_tx, "ephemeral_signer", index_le_u8]`.
    // Squads' on-chain `vault_transaction_create` rejects >255 ephemeral
    // signers via the same `as u8` cast, so any vault_tx we read back
    // here will fit; surface a typed error if a forged account ever
    // doesn't, instead of silently producing wrong PDAs.
    let ephemeral_signers: Vec<Pubkey> = vault_tx
        .ephemeral_signer_bumps
        .iter()
        .enumerate()
        .map(|(i, _bump)| {
            let idx = SquadsError::try_u8(i, MessageField::EphemeralSignerIndex)?;
            Ok::<Pubkey, Error>(
                Pubkey::find_program_address(
                    &[
                        SEED_PREFIX,
                        vault_tx_addr.as_ref(),
                        b"ephemeral_signer",
                        &idx.to_le_bytes(),
                    ],
                    &PROGRAM_ID,
                )
                .0,
            )
        })
        .collect::<Result<_, _>>()?;

    let mut remaining: Vec<AccountMeta> =
        Vec::with_capacity(msg.account_keys.len() + msg.address_table_lookups.len() * 2 + 4);

    // 1. LUT accounts (read-only references to the tables themselves).
    for lookup in msg.address_table_lookups.iter() {
        remaining.push(AccountMeta::new_readonly(lookup.account_key, false));
    }

    // 2. Static account_keys with their writable/signer flags from the
    //    message. Vault and ephemeral-signer PDAs sign via invoke_signed
    //    inside the program, not the outer tx, so strip is_signer for them.
    for (i, key) in msg.account_keys.iter().enumerate() {
        let writable = msg.is_static_writable_index(i);
        let signer = msg.is_signer_index(i)
            && key != vault_addr.as_pubkey()
            && !ephemeral_signers.contains(key);
        remaining.push(AccountMeta {
            pubkey: *key,
            is_signer: signer,
            is_writable: writable,
        });
    }

    // 3. LUT-resolved entries per LUT: writable then readonly.
    for (writable_keys, readonly_keys) in lut_resolutions {
        for pk in writable_keys {
            remaining.push(AccountMeta::new(*pk, false));
        }
        for pk in readonly_keys {
            remaining.push(AccountMeta::new_readonly(*pk, false));
        }
    }

    let mut accounts = vec![
        AccountMeta::new_readonly(multisig.into_pubkey(), false),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(vault_tx_addr, false),
        AccountMeta::new_readonly(member, true),
    ];
    accounts.extend(remaining);

    Ok(Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data: VAULT_TRANSACTION_EXECUTE_IX.to_vec(),
    })
}

/// Build the right v4 execute instruction for the proposal at
/// `(multisig, transaction_index)` — VaultTransaction or
/// ConfigTransaction. Reads the on-chain transaction account once to
/// dispatch by 8-byte Anchor discriminator. Caller must hold the
/// keypair for `member` and the member must have `Execute` permission.
pub async fn execute_ix<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: MultisigKey,
    transaction_index: u64,
    member: Pubkey,
) -> Result<solana_sdk::instruction::Instruction, Error> {
    let tx_addr = vault_transaction_pda(&multisig, transaction_index);
    let account = client.as_ref().get_account(&tx_addr).await?;
    if account.owner != PROGRAM_ID {
        return Err(DecodeError::wrong_owner(&tx_addr, "Squads v4", &account.owner).into());
    }
    let disc = super::read_discriminator(&account.data)
        .ok_or_else(|| DecodeError::wrong_discriminator(&tx_addr, "v4 transaction"))?;
    match disc {
        VAULT_TRANSACTION_DISCRIMINATOR => {
            vault_transaction_execute_ix(client, multisig, transaction_index, member).await
        }
        CONFIG_TRANSACTION_DISCRIMINATOR => Ok(config_transaction_execute_ix(
            multisig,
            transaction_index,
            member,
        )),
        _ => Err(DecodeError::wrong_discriminator(
            &tx_addr,
            "v4 VaultTransaction or ConfigTransaction",
        )
        .into()),
    }
}

/// Build a `config_transaction_execute` instruction. Applies the
/// pending member / threshold / time-lock / spending-limit changes to
/// the multisig itself. The caller's `member` pubkey doubles as the
/// `rent_payer` (Squads programs reallocate the multisig account when
/// `members` grows or shrinks, and the rent payer covers the delta).
fn config_transaction_execute_ix(
    multisig: MultisigKey,
    transaction_index: u64,
    member: Pubkey,
) -> solana_sdk::instruction::Instruction {
    use solana_sdk::instruction::{AccountMeta, Instruction};

    let proposal = proposal_pda(&multisig, transaction_index);
    let transaction = vault_transaction_pda(&multisig, transaction_index);

    let accounts = vec![
        AccountMeta::new(multisig.into_pubkey(), false),
        AccountMeta::new_readonly(member, true),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(transaction, false),
        AccountMeta::new(member, true),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ];

    Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data: CONFIG_TRANSACTION_EXECUTE_IX.to_vec(),
    }
}

/// Fetch the multisig's `time_lock` setting (seconds between proposal
/// approval and the earliest legal execute). Callers that want to bundle
/// `proposal_approve + *_transaction_execute` into a single transaction
/// need this to be `0` — the execute handler checks `now - approval_ts
/// >= time_lock`, which never holds inside the same block.
pub async fn get_time_lock<C: AsRef<SolanaRpcClient>>(
    client: &C,
    multisig: &MultisigKey,
) -> Result<u32, Error> {
    let state: squads_multisig_program::state::Multisig = fetch_account(
        client.as_ref(),
        multisig.as_pubkey(),
        &MULTISIG_DISCRIMINATOR,
        "v4 Multisig",
    )
    .await?;
    Ok(state.time_lock)
}

/// All three v4 vote instructions share the same accounts shape and
/// `ProposalVoteArgs { memo: Option<String> }` body — only the
/// discriminator differs. See upstream `proposal_vote.rs` for the
/// definitions and the validation rules each one enforces on-chain.
fn proposal_vote_ix(
    multisig: MultisigKey,
    transaction_index: u64,
    member: Pubkey,
    memo: Option<String>,
    discriminator: &[u8; 8],
) -> Result<solana_sdk::instruction::Instruction, Error> {
    use anchor_lang::AnchorSerialize;
    use solana_sdk::instruction::{AccountMeta, Instruction};

    let args = squads_multisig_program::instructions::ProposalVoteArgs { memo };
    let mut data = discriminator.to_vec();
    args.serialize(&mut data)
        .map_err(|e| EncodeError::borsh("ProposalVoteArgs", e))?;

    let proposal = proposal_pda(&multisig, transaction_index);
    Ok(Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(multisig.into_pubkey(), false),
            AccountMeta::new(member, true),
            AccountMeta::new(proposal, false),
        ],
        data,
    })
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

    /// Round-trip: an approve instruction's first 8 data bytes are the
    /// approve discriminator, the rest deserializes into a
    /// `ProposalVoteArgs` carrying the original memo. Catches mismatches
    /// between the hardcoded discriminators and the upstream args type.
    #[test]
    fn proposal_approve_roundtrip() {
        let multisig = MultisigKey::from_pubkey(Pubkey::new_unique());
        let member = Pubkey::new_unique();
        let memo = Some("ship it".to_string());
        let ix = proposal_approve_ix(multisig, 42, member, memo.clone()).expect("build");

        assert_eq!(ix.program_id, PROGRAM_ID);
        assert_eq!(&ix.data[..8], &PROPOSAL_APPROVE_IX);
        let args = squads_multisig_program::instructions::ProposalVoteArgs::deserialize(
            &mut &ix.data[8..],
        )
        .expect("decode args");
        assert_eq!(args.memo, memo);

        // Account ordering: multisig (ro), member (signer mut), proposal (mut)
        assert_eq!(ix.accounts.len(), 3);
        assert_eq!(ix.accounts[0].pubkey, multisig.into_pubkey());
        assert!(!ix.accounts[0].is_signer);
        assert!(!ix.accounts[0].is_writable);
        assert_eq!(ix.accounts[1].pubkey, member);
        assert!(ix.accounts[1].is_signer);
        assert!(ix.accounts[1].is_writable);
        assert_eq!(ix.accounts[2].pubkey, proposal_pda(&multisig, 42));
        assert!(!ix.accounts[2].is_signer);
        assert!(ix.accounts[2].is_writable);
    }

    /// Reject and cancel share the same accounts shape; only the
    /// discriminator differs.
    #[test]
    fn vote_discriminators_distinct() {
        assert_ne!(PROPOSAL_APPROVE_IX, PROPOSAL_REJECT_IX);
        assert_ne!(PROPOSAL_APPROVE_IX, PROPOSAL_CANCEL_IX);
        assert_ne!(PROPOSAL_REJECT_IX, PROPOSAL_CANCEL_IX);
    }

    /// Compile a tiny synthetic instruction message and confirm the
    /// header counts, key ordering, and payload framing match Squads'
    /// expected layout. Catches regressions in the SmallVec encoding.
    #[test]
    fn compile_message_layout() {
        use solana_sdk::instruction::{AccountMeta, Instruction};
        let vault = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(vault, true),
                AccountMeta::new(recipient, false),
            ],
            data: vec![1, 2, 3, 4],
        };
        let bytes = compile_transaction_message(&vault, &[ix]).expect("compact");

        // Header: 1 signer total, 1 writable signer, 1 writable non-signer
        // (recipient), and program_id is readonly non-signer.
        assert_eq!(bytes[0], 1, "num_signers");
        assert_eq!(bytes[1], 1, "num_writable_signers");
        assert_eq!(bytes[2], 1, "num_writable_non_signers");

        // account_keys: u8 length = 3 (vault, recipient, program), then
        // 3 × 32 bytes. Vault must be at index 0 (writable signer slot).
        assert_eq!(bytes[3], 3, "account_keys count");
        let key0 = &bytes[4..36];
        assert_eq!(key0, vault.as_ref(), "vault first");

        // Instruction count = 1 immediately after the keys section.
        let ix_count_offset = 4 + 3 * 32;
        assert_eq!(bytes[ix_count_offset], 1, "instruction count");

        // CompiledInstruction: program_id_index (u8), account_indexes
        // SmallVec<u8, u8> = u8 len + N bytes, data SmallVec<u16, u8> =
        // u16 LE len + N bytes.
        let p = ix_count_offset + 1;
        // program_id_index points to the program in account_keys.
        assert_eq!(bytes[p], 2, "program_id at index 2");
        assert_eq!(bytes[p + 1], 2, "account_indexes len = 2");
        // account_indexes[0] = vault (0), [1] = recipient (1)
        assert_eq!(bytes[p + 2], 0);
        assert_eq!(bytes[p + 3], 1);
        // data length is u16 LE = 4
        assert_eq!(&bytes[p + 4..p + 6], &4u16.to_le_bytes());
        assert_eq!(&bytes[p + 6..p + 10], &[1, 2, 3, 4]);

        // Trailing empty address_table_lookups (SmallVec<u8, _> length = 0).
        assert_eq!(*bytes.last().unwrap(), 0, "no LUTs");
    }

    #[test]
    fn create_discriminators_distinct() {
        assert_ne!(VAULT_TRANSACTION_CREATE_IX, PROPOSAL_CREATE_IX);
        assert_ne!(VAULT_TRANSACTION_CREATE_IX, VAULT_TRANSACTION_EXECUTE_IX);
        assert_ne!(VAULT_TRANSACTION_CREATE_IX, CONFIG_TRANSACTION_CREATE_IX);
        assert_ne!(CONFIG_TRANSACTION_CREATE_IX, PROPOSAL_CREATE_IX);
    }

    /// Round-trip: a config_transaction_create instruction's data
    /// starts with the discriminator and decodes into
    /// `ConfigTransactionCreateArgs` carrying the actions and memo
    /// passed in. Catches discriminator typos and args-shape drift.
    #[test]
    fn config_transaction_create_roundtrip() {
        use squads_multisig_program::state::{ConfigAction, Member, Permissions};
        let multisig = MultisigKey::from_pubkey(Pubkey::new_unique());
        let creator = Pubkey::new_unique();
        let new_member = Pubkey::new_unique();
        let actions = vec![
            ConfigAction::AddMember {
                new_member: Member {
                    key: new_member,
                    permissions: Permissions { mask: 0b111 },
                },
            },
            ConfigAction::ChangeThreshold { new_threshold: 2 },
        ];
        let memo = Some("add member".to_string());
        let ix = config_transaction_create_ix(multisig, 5, creator, actions, memo.clone())
            .expect("build");

        assert_eq!(ix.program_id, PROGRAM_ID);
        assert_eq!(&ix.data[..8], &CONFIG_TRANSACTION_CREATE_IX);
        let args = squads_multisig_program::instructions::ConfigTransactionCreateArgs::deserialize(
            &mut &ix.data[8..],
        )
        .expect("decode args");
        assert_eq!(args.actions.len(), 2);
        assert_eq!(args.memo, memo);

        // Account ordering: multisig (mut), transaction (mut), creator
        // (signer), rent_payer (signer mut, same as creator), system_program.
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(ix.accounts[0].pubkey, multisig.into_pubkey());
        assert!(ix.accounts[0].is_writable);
        assert_eq!(ix.accounts[1].pubkey, vault_transaction_pda(&multisig, 5));
        assert!(ix.accounts[1].is_writable);
        assert_eq!(ix.accounts[2].pubkey, creator);
        assert!(ix.accounts[2].is_signer);
        assert_eq!(ix.accounts[3].pubkey, creator); // rent_payer = creator
        assert!(ix.accounts[3].is_signer);
        assert!(ix.accounts[3].is_writable);
        assert_eq!(ix.accounts[4].pubkey, solana_sdk::system_program::ID);
    }

    /// `ConfigActionInput::AddMember` round-trips into the upstream
    /// `state::ConfigAction::AddMember` with the right Member +
    /// Permissions shape, and `MemberPermissions::to_mask` produces
    /// the bit positions Squads' on-chain `Permission` enum expects.
    #[test]
    fn config_action_input_add_member_lowering() {
        use squads_multisig_program::state::ConfigAction;
        let new_member = Pubkey::new_unique();
        let action: ConfigAction = ConfigActionInput::AddMember {
            new_member,
            // Initiate + Execute, not Vote.
            permissions: MemberPermissions {
                propose: true,
                vote: false,
                execute: true,
            },
        }
        .into();
        match action {
            ConfigAction::AddMember { new_member: m } => {
                assert_eq!(m.key, new_member);
                assert_eq!(m.permissions.mask, 0b101);
            }
            _ => panic!("expected AddMember"),
        }
    }

    /// `MemberPermissions::to_mask` ↔ `from_mask` round-trip pins the
    /// bit-position contract with Squads' upstream `Permission` enum
    /// (`Initiate=1<<0`, `Vote=1<<1`, `Execute=1<<2`). A regression in
    /// either direction surfaces here.
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
        // Spot-check known bit positions.
        assert_eq!(MemberPermissions::ALL.to_mask(), 0b111);
        assert_eq!(MemberPermissions::default().to_mask(), 0);
    }

    /// An account in a LUT is pulled out of the static keys, encoded as
    /// a lookup reference (account_key + index inside the LUT), and
    /// referenced by the compiled instruction at
    /// `static_keys.len() + offset` in the resolved section.
    #[test]
    fn compile_message_with_luts_layout() {
        use solana_sdk::address_lookup_table::AddressLookupTableAccount;
        use solana_sdk::instruction::{AccountMeta, Instruction};

        let vault = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let lut_account = Pubkey::new_unique();
        let lut = AddressLookupTableAccount {
            key: Pubkey::new_unique(),
            // Pad with a few unrelated entries so addr_idx isn't 0.
            addresses: vec![
                Pubkey::new_unique(),
                Pubkey::new_unique(),
                lut_account,
                Pubkey::new_unique(),
            ],
        };
        let lut_addr_idx: u8 = 2;

        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(vault, true),
                AccountMeta::new(recipient, false),
                AccountMeta::new_readonly(lut_account, false),
            ],
            data: vec![9, 9, 9],
        };
        let bytes =
            compile_transaction_message_with_luts(&vault, &[ix], std::slice::from_ref(&lut))
                .expect("compact");

        // Header.
        assert_eq!(bytes[0], 1, "num_signers");
        assert_eq!(bytes[1], 1, "num_writable_signers");
        assert_eq!(bytes[2], 1, "num_writable_non_signers");

        // Static keys: only vault, recipient, program — lut_account is
        // resolved through the LUT, not inlined.
        assert_eq!(bytes[3], 3, "static account_keys count");
        let key0 = &bytes[4..36];
        assert_eq!(key0, vault.as_ref(), "vault first");

        // Instruction section.
        let ix_count_offset = 4 + 3 * 32;
        assert_eq!(bytes[ix_count_offset], 1);
        let p = ix_count_offset + 1;
        // program_id sits in static_keys at index 2 (readonly non-signer).
        assert_eq!(bytes[p], 2, "program_id_index = 2");
        assert_eq!(bytes[p + 1], 3, "account_indexes len = 3");
        assert_eq!(bytes[p + 2], 0, "vault at static index 0");
        assert_eq!(bytes[p + 3], 1, "recipient at static index 1");
        // lut_account resolves at static_keys.len() + 0 = 3 (first
        // entry in the LUT's resolved section, readonly).
        assert_eq!(bytes[p + 4], 3, "lut_account at index 3");
        assert_eq!(&bytes[p + 5..p + 7], &3u16.to_le_bytes(), "data len");
        assert_eq!(&bytes[p + 7..p + 10], &[9, 9, 9]);

        // address_table_lookups: 1 entry, account_key, no writable, one
        // readonly index pointing into the LUT.
        let lut_offset = p + 10;
        assert_eq!(bytes[lut_offset], 1, "1 LUT lookup");
        assert_eq!(&bytes[lut_offset + 1..lut_offset + 33], lut.key.as_ref());
        assert_eq!(bytes[lut_offset + 33], 0, "writable_indexes empty");
        assert_eq!(bytes[lut_offset + 34], 1, "readonly_indexes len = 1");
        assert_eq!(bytes[lut_offset + 35], lut_addr_idx);
        assert_eq!(bytes.len(), lut_offset + 36, "no trailing bytes");
    }

    /// Counts that don't fit Squads' u8/u16 wire-format limits should
    /// surface a clean `MessageFieldOverflow` error rather than silently
    /// truncate via `as u8` and produce a corrupted message that
    /// on-chain validation would reject with an unrelated parse error.
    #[test]
    fn compile_message_rejects_oversized_data() {
        use solana_sdk::instruction::{AccountMeta, Instruction};
        let vault = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let ix = Instruction {
            program_id: program,
            accounts: vec![AccountMeta::new(vault, true)],
            // Just over u16::MAX bytes of data.
            data: vec![0u8; usize::from(u16::MAX) + 1],
        };
        let err =
            compile_transaction_message(&vault, &[ix]).expect_err("must reject oversized data");
        let msg = err.to_string();
        assert!(
            msg.contains("instruction_data_len"),
            "error should name the offending field, got: {msg}",
        );
    }

    /// Same input must produce the same byte output across runs — both
    /// the static-key buckets and the per-LUT entries pull from
    /// HashMaps internally, so ordering needs an explicit sort. A
    /// proposal payload that hashes differently between runs would
    /// break any external tooling that pins proposals by content hash.
    #[test]
    fn compile_message_is_deterministic() {
        use solana_sdk::address_lookup_table::AddressLookupTableAccount;
        use solana_sdk::instruction::{AccountMeta, Instruction};

        let vault = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        // Eight distinct accounts spread across all four buckets so
        // HashMap ordering would visibly affect output.
        let writable_signer = Pubkey::new_unique();
        let readonly_signer = Pubkey::new_unique();
        let writable_a = Pubkey::new_unique();
        let writable_b = Pubkey::new_unique();
        let readonly_a = Pubkey::new_unique();
        let readonly_b = Pubkey::new_unique();
        let lut_a = Pubkey::new_unique();
        let lut_b = Pubkey::new_unique();
        let lut = AddressLookupTableAccount {
            key: Pubkey::new_unique(),
            addresses: vec![lut_a, lut_b],
        };
        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(vault, true),
                AccountMeta::new(writable_signer, true),
                AccountMeta::new_readonly(readonly_signer, true),
                AccountMeta::new(writable_a, false),
                AccountMeta::new(writable_b, false),
                AccountMeta::new_readonly(readonly_a, false),
                AccountMeta::new_readonly(readonly_b, false),
                AccountMeta::new(lut_a, false),
                AccountMeta::new_readonly(lut_b, false),
            ],
            data: vec![7; 16],
        };
        let first = compile_transaction_message_with_luts(
            &vault,
            std::slice::from_ref(&ix),
            std::slice::from_ref(&lut),
        )
        .expect("compact");
        for _ in 0..16 {
            assert_eq!(
                first,
                compile_transaction_message_with_luts(
                    &vault,
                    std::slice::from_ref(&ix),
                    std::slice::from_ref(&lut)
                )
                .expect("compact"),
                "compactor output must be deterministic"
            );
        }
    }

    /// Empty `luts` is a no-op: output equals the non-LUT compactor.
    #[test]
    fn compile_message_with_empty_luts_matches_static() {
        use solana_sdk::instruction::{AccountMeta, Instruction};
        let vault = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(vault, true),
                AccountMeta::new(recipient, false),
            ],
            data: vec![1, 2, 3, 4],
        };
        let static_only =
            compile_transaction_message(&vault, std::slice::from_ref(&ix)).expect("compact");
        let with_empty_luts =
            compile_transaction_message_with_luts(&vault, &[ix], &[]).expect("compact");
        assert_eq!(static_only, with_empty_luts);
    }

    /// A signer in an inner instruction must remain in the static keys
    /// even if it appears in a LUT — LUTs can't carry signatures.
    #[test]
    fn compile_message_signer_forced_static_despite_lut() {
        use solana_sdk::address_lookup_table::AddressLookupTableAccount;
        use solana_sdk::instruction::{AccountMeta, Instruction};

        let vault = Pubkey::new_unique();
        let extra_signer = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let lut = AddressLookupTableAccount {
            key: Pubkey::new_unique(),
            addresses: vec![extra_signer],
        };
        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(vault, true),
                AccountMeta::new_readonly(extra_signer, true),
            ],
            data: vec![],
        };
        let bytes = compile_transaction_message_with_luts(&vault, &[ix], &[lut]).expect("compact");

        // Both signers stay static: 2 signers, 1 writable signer.
        assert_eq!(bytes[0], 2, "num_signers");
        assert_eq!(bytes[1], 1, "num_writable_signers");
        assert_eq!(bytes[3], 3, "static account_keys count");
        // No LUT entries should be emitted (the signer was the only
        // candidate, and signers are forced static).
        let last = *bytes.last().unwrap();
        assert_eq!(last, 0, "no LUT lookups emitted");
    }

    /// When an account appears in two LUTs the compactor must pick the
    /// first — the inner `position` + `break` combination at the LUT
    /// classification site enforces this. A future refactor that
    /// scanned all LUTs (or picked the last) would silently switch
    /// which LUT a proposal references; the on-chain `address_table_lookups`
    /// section then points at a different table than the proposer
    /// intended. Test pins the contract.
    #[test]
    fn compile_message_lut_first_wins() {
        use solana_sdk::address_lookup_table::AddressLookupTableAccount;
        use solana_sdk::instruction::{AccountMeta, Instruction};

        let vault = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        let shared = Pubkey::new_unique();
        let lut_a = AddressLookupTableAccount {
            key: Pubkey::new_unique(),
            addresses: vec![shared],
        };
        let lut_b = AddressLookupTableAccount {
            key: Pubkey::new_unique(),
            addresses: vec![shared],
        };
        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(vault, true),
                AccountMeta::new_readonly(shared, false),
            ],
            data: vec![],
        };
        let bytes = compile_transaction_message_with_luts(&vault, &[ix], &[lut_a.clone(), lut_b])
            .expect("compact");

        // Static count = 2 (vault + program); shared is LUT-resolved.
        assert_eq!(bytes[3], 2, "static account_keys count");
        // Walk to the address_table_lookups section: header(3) + static
        // keys (1 + 2*32) + ix section (1 + 1 + 1 + 2 + 2 + 0).
        let static_end = 4 + 2 * 32;
        // Instruction section: count(1) + program_id_index(1) +
        // accounts_len(1) + 2 account indexes + data_len(2) + 0 bytes.
        let ix_count_offset = static_end;
        assert_eq!(bytes[ix_count_offset], 1);
        let lut_section_offset = ix_count_offset + 1 + 1 + 1 + 2 + 2;
        assert_eq!(bytes[lut_section_offset], 1, "exactly one LUT used");
        // First 32 bytes of the LUT entry must be lut_a's key — first wins.
        assert_eq!(
            &bytes[lut_section_offset + 1..lut_section_offset + 33],
            lut_a.key.as_ref(),
            "first LUT in the slice must win",
        );
    }

    /// Per-LUT writable entries must precede readonly entries in the
    /// resolved index space — Squads' `ExecutableTransactionMessage::
    /// new_validated` walks the section in that order. Test feeds one
    /// LUT containing both a writable and a readonly account and
    /// asserts the writable account's index comes first in the
    /// compiled instruction's `account_indexes`.
    #[test]
    fn compile_message_lut_writable_before_readonly() {
        use solana_sdk::address_lookup_table::AddressLookupTableAccount;
        use solana_sdk::instruction::{AccountMeta, Instruction};

        let vault = Pubkey::new_unique();
        let program = Pubkey::new_unique();
        // Pick fixed bytes so deterministic-sort doesn't reshuffle them.
        let writable_key = Pubkey::from([1u8; 32]);
        let readonly_key = Pubkey::from([2u8; 32]);
        let lut = AddressLookupTableAccount {
            key: Pubkey::new_unique(),
            addresses: vec![readonly_key, writable_key], // intentionally not in order
        };
        let ix = Instruction {
            program_id: program,
            accounts: vec![
                AccountMeta::new(vault, true),
                AccountMeta::new(writable_key, false),
                AccountMeta::new_readonly(readonly_key, false),
            ],
            data: vec![],
        };
        let bytes =
            compile_transaction_message_with_luts(&vault, &[ix], std::slice::from_ref(&lut))
                .expect("compact");

        // Static count = 2 (vault + program); both LUT keys are resolved.
        let static_count = bytes[3] as usize;
        assert_eq!(static_count, 2, "static account_keys count");

        // Walk to the LUT section.
        let ix_count_offset = 4 + static_count * 32;
        assert_eq!(bytes[ix_count_offset], 1);
        let p = ix_count_offset + 1;
        // Instruction account_indexes layout: vault(0), writable_key, readonly_key.
        assert_eq!(bytes[p + 1], 3, "account_indexes len = 3");
        assert_eq!(bytes[p + 2], 0, "vault at static index 0");
        // Writable goes first in the LUT-resolved section.
        let writable_resolved_idx = static_count as u8;
        let readonly_resolved_idx = static_count as u8 + 1;
        assert_eq!(
            bytes[p + 3],
            writable_resolved_idx,
            "writable_key at static_count+0",
        );
        assert_eq!(
            bytes[p + 4],
            readonly_resolved_idx,
            "readonly_key at static_count+1",
        );

        // Verify the address_table_lookups layout: writable_indexes
        // section comes before readonly_indexes. Layout after `p`:
        // program_id_index(1) + accounts_len(1) + 3 indexes + data_len(2)
        // + 0 data bytes = p + 7.
        let lut_section_offset = p + 7;
        assert_eq!(bytes[lut_section_offset], 1, "1 LUT");
        let lut_offset = lut_section_offset + 1;
        // 32 bytes of LUT account_key, then writable_indexes len + 1 byte,
        // then readonly_indexes len + 1 byte.
        assert_eq!(bytes[lut_offset + 32], 1, "writable_indexes len = 1");
        assert_eq!(
            bytes[lut_offset + 33],
            1, // index 1 in the LUT (writable_key)
            "writable_indexes[0] points at writable_key in LUT addresses",
        );
        assert_eq!(bytes[lut_offset + 34], 1, "readonly_indexes len = 1");
        assert_eq!(
            bytes[lut_offset + 35],
            0, // index 0 in the LUT (readonly_key)
            "readonly_indexes[0] points at readonly_key in LUT addresses",
        );
    }

    /// `vault_transaction_execute` mirror of the compactor's
    /// LUT-aware layout: outer instruction's `remaining_accounts` must
    /// be assembled in the strict three-section order Squads' on-chain
    /// validator expects:
    ///   1. LUT account_keys (read-only references to the tables)
    ///   2. Static `message.account_keys` in their original order, with
    ///      the writable/signer flags from the message — except vault
    ///      and ephemeral-signer PDAs lose `is_signer` because they
    ///      sign via `invoke_signed` inside the program.
    ///   3. For each LUT in `address_table_lookups` order: the LUT's
    ///      writable-resolved keys (mutable, non-signer) followed by
    ///      its readonly-resolved keys (immutable, non-signer).
    ///
    /// A regression in any of these — section ordering, signer-strip,
    /// per-LUT writable-then-readonly ordering — would produce a
    /// misaligned account list that the on-chain Squads program would
    /// reject (or, worse, silently mis-apply). Pin all three sections
    /// here using a synthetic VaultTransaction with one ephemeral
    /// signer and one LUT, no RPC.
    #[test]
    fn assemble_vault_execute_ix_three_section_layout() {
        use solana_sdk::instruction::AccountMeta;
        use squads_multisig_program::state::{
            MultisigCompiledInstruction, MultisigMessageAddressTableLookup, VaultTransaction,
            VaultTransactionMessage,
        };

        let multisig = MultisigKey::from_pubkey(Pubkey::new_unique());
        let transaction_index: u64 = 42;
        let member = Pubkey::new_unique();
        let vault_tx_addr = vault_transaction_pda(&multisig, transaction_index);
        let vault_index: u8 = 0;
        let vault_addr = vault_pda(&multisig, vault_index);

        // One ephemeral signer at index 0; its PDA must lose
        // `is_signer` even though the message marks it a signer.
        let ephemeral_signer = Pubkey::find_program_address(
            &[
                SEED_PREFIX,
                vault_tx_addr.as_ref(),
                b"ephemeral_signer",
                &0u8.to_le_bytes(),
            ],
            &PROGRAM_ID,
        )
        .0;

        // Static keys layout: vault (writable signer at 0), eph_signer
        // (readonly signer), program (readonly non-signer), recipient
        // (writable non-signer). The compactor orders the message
        // header counts as (num_signers=2, num_writable_signers=1,
        // num_writable_non_signers=1).
        let program = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let static_keys = vec![
            vault_addr.into_pubkey(),
            ephemeral_signer,
            program,
            recipient,
        ];
        // One LUT with two writable + one readonly resolved entry.
        let lut_addr = Pubkey::new_unique();
        let writable_resolved = vec![Pubkey::new_unique(), Pubkey::new_unique()];
        let readonly_resolved = vec![Pubkey::new_unique()];

        let vault_tx = VaultTransaction {
            multisig: multisig.into_pubkey(),
            creator: Pubkey::new_unique(),
            index: transaction_index,
            bump: 0,
            vault_index,
            vault_bump: 0,
            ephemeral_signer_bumps: vec![0u8],
            message: VaultTransactionMessage {
                num_signers: 2,
                num_writable_signers: 1,
                num_writable_non_signers: 1,
                account_keys: static_keys.clone(),
                instructions: vec![MultisigCompiledInstruction {
                    program_id_index: 2, // program
                    account_indexes: vec![0, 3],
                    data: vec![],
                }],
                address_table_lookups: vec![MultisigMessageAddressTableLookup {
                    account_key: lut_addr,
                    writable_indexes: vec![0u8, 1u8],
                    readonly_indexes: vec![2u8],
                }],
            },
        };

        let lut_resolutions = vec![(writable_resolved.clone(), readonly_resolved.clone())];
        let ix = assemble_vault_execute_ix(
            multisig,
            transaction_index,
            member,
            &vault_tx,
            &lut_resolutions,
        )
        .expect("assemble");

        assert_eq!(ix.program_id, PROGRAM_ID);
        assert_eq!(&ix.data[..], &VAULT_TRANSACTION_EXECUTE_IX);

        // Outer fixed prefix: multisig (ro), proposal (mut), vault_tx (ro), member (signer ro).
        let proposal = proposal_pda(&multisig, transaction_index);
        assert_eq!(
            ix.accounts[0],
            AccountMeta::new_readonly(multisig.into_pubkey(), false),
        );
        assert_eq!(ix.accounts[1], AccountMeta::new(proposal, false));
        assert_eq!(
            ix.accounts[2],
            AccountMeta::new_readonly(vault_tx_addr, false),
        );
        assert_eq!(ix.accounts[3], AccountMeta::new_readonly(member, true));

        // Section 1: LUT accounts (read-only references). One LUT.
        let mut cursor = 4;
        assert_eq!(
            ix.accounts[cursor],
            AccountMeta::new_readonly(lut_addr, false),
            "section 1: LUT account_keys come first",
        );
        cursor += 1;

        // Section 2: static account_keys in their message order, with
        // signer-strip applied to vault and ephemeral signers. Vault
        // is writable per num_writable_signers=1, but loses is_signer.
        // Ephemeral signer is readonly per the layout (signer count 2,
        // writable signers 1) and loses is_signer too.
        assert_eq!(
            ix.accounts[cursor].pubkey,
            vault_addr.into_pubkey(),
            "section 2[0] is vault",
        );
        assert!(ix.accounts[cursor].is_writable, "vault stays writable");
        assert!(
            !ix.accounts[cursor].is_signer,
            "vault loses is_signer at outer level (signed via invoke_signed)",
        );
        cursor += 1;

        assert_eq!(
            ix.accounts[cursor].pubkey, ephemeral_signer,
            "section 2[1] is ephemeral signer",
        );
        assert!(
            !ix.accounts[cursor].is_signer,
            "ephemeral signer loses is_signer at outer level",
        );
        cursor += 1;

        // Program and recipient are non-signers; flags carry through.
        assert_eq!(ix.accounts[cursor].pubkey, program);
        assert!(!ix.accounts[cursor].is_signer);
        cursor += 1;
        assert_eq!(ix.accounts[cursor].pubkey, recipient);
        cursor += 1;

        // Section 3: per-LUT writable resolved (mutable non-signers),
        // then readonly resolved (immutable non-signers). Order
        // matters strictly.
        for pk in &writable_resolved {
            assert_eq!(
                ix.accounts[cursor],
                AccountMeta::new(*pk, false),
                "section 3 writable-resolved must precede readonly-resolved",
            );
            cursor += 1;
        }
        for pk in &readonly_resolved {
            assert_eq!(
                ix.accounts[cursor],
                AccountMeta::new_readonly(*pk, false),
                "section 3 readonly-resolved follows writable",
            );
            cursor += 1;
        }

        // No accounts past section 3.
        assert_eq!(
            cursor,
            ix.accounts.len(),
            "no trailing accounts past section 3",
        );
    }

    /// `resolve_lut_indexes` is the load-bearing helper for the
    /// critical LUT-out-of-range bug fix: a silently-dropped index
    /// would shift later resolved entries up by one slot and produce
    /// a misaligned `remaining_accounts` list. Pin both arms — every
    /// in-range index resolves to its address, and any out-of-range
    /// index bails with a typed `LutIndexOutOfRange`.
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

        // In-range indexes resolve in order.
        let resolved =
            resolve_lut_indexes(&table, &table_addr, &[2u8, 0u8]).expect("in-range resolves");
        assert_eq!(resolved, vec![addrs[2], addrs[0]]);

        // Empty input is a clean empty result.
        assert_eq!(
            resolve_lut_indexes(&table, &table_addr, &[]).expect("empty"),
            Vec::<Pubkey>::new(),
        );

        // Any out-of-range index bails — pin the typed error so a
        // future regression to silent-skip surfaces here.
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
