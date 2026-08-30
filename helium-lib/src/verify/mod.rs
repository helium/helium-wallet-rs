//! Checks a caller applies to a transaction built somewhere else.
//!
//! A transaction arriving from a remote builder is bytes the caller is about to
//! authorize, so what it may contain is the caller's policy, not the builder's.
//! These are the checks that policy is expressed with: each one is a single
//! question about a decoded transaction, and callers compose them with whatever
//! else they know about the action they asked for.
//!
//! Available without the `txn` feature, since a caller that no longer builds
//! transactions still has to inspect them.

mod action;
pub use action::*;

use std::collections::HashMap;

use crate::{
    blockchain_api::types::SwapQuote,
    keypair::{pubkey, Pubkey},
    programs::KnownProgram,
    solana_sdk::instruction::CompiledInstruction,
    transaction::VersionedTransaction,
};

/// Compute-unit limit a transaction cannot exceed, set by the runtime.
pub const MAX_COMPUTE_UNIT_LIMIT: u64 = 1_400_000;

/// Compute units the runtime assumes per instruction when no limit is set.
const DEFAULT_COMPUTE_UNITS_PER_IX: u64 = 200_000;

/// A transaction that fails a check.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// A program or account index points past the static keys, so it resolves
    /// through a lookup table and cannot be checked.
    #[error("{what} is loaded from a lookup table and cannot be verified")]
    LookupTable { what: &'static str },
    /// A program outside the allowed set is invoked.
    #[error("transaction invokes {program}, which is not expected here")]
    UnexpectedProgram { program: Pubkey },
    /// More than one signature is required, or none.
    #[error("transaction requires {required} signatures, expected exactly one")]
    SignerCount { required: u8 },
    /// The sole required signer is not the expected wallet.
    #[error("transaction must be signed by {expected}, not {actual:?}")]
    WrongSigner {
        expected: Pubkey,
        actual: Option<Pubkey>,
    },
    /// The prioritization fee exceeds what the caller allows.
    #[error("transaction prices compute at up to {lamports} lamports, above the {cap} allowed")]
    PriorityFee { lamports: u64, cap: u64 },
    /// The transaction is anchored to a durable nonce, so it never expires.
    #[error("transaction is anchored to a durable nonce and would never expire")]
    NonceAnchored,
    /// An allowed program is invoked with an instruction that is not. `tag` is
    /// the instruction's leading discriminator byte, absent when it carries no
    /// data, and is what names the offending call in a refusal an operator
    /// reads without the transaction in front of them.
    #[error("{program} is invoked with instruction {tag:?}, which is not expected here")]
    UnexpectedInstruction { program: Pubkey, tag: Option<u8> },
    /// The transaction carries a different number of the requested action than
    /// the one it was asked for. Zero is the important case: an action that was
    /// never built otherwise reads the same as one built correctly.
    #[error("expected exactly one {action}, found {found}")]
    ActionCount { action: &'static str, found: usize },
    /// An account the action names is not the one that was requested.
    #[error("{action} names {actual} as its {role}, not the requested {expected}")]
    ActionAccount {
        action: &'static str,
        role: &'static str,
        expected: Pubkey,
        actual: Pubkey,
    },
    /// A value in the action's arguments is not the one that was requested.
    #[error("{action} carries {actual} as its {field}, not the requested {expected}")]
    ActionValue {
        action: &'static str,
        field: &'static str,
        expected: String,
        actual: String,
    },
    /// The action sets a field the caller did not ask it to.
    #[error("{action} sets {field}, which was not requested")]
    ActionUnrequested {
        action: &'static str,
        field: &'static str,
    },
    /// The action's arguments do not decode against the program's IDL, so a
    /// check against them cannot be made. Unreadable is not absent.
    #[error("{action} arguments could not be read")]
    ActionUnreadable { action: &'static str },
    /// A proposal carries an action at the top level that it should only wrap.
    #[error("a proposal carries a top-level {action} instead of wrapping it")]
    ActionNotWrapped { action: &'static str },
    /// A builder returned a different number of transactions than the single
    /// one the action was expected to need.
    #[error("expected one transaction, got {found}")]
    TransactionCount { found: usize },
    /// A quote does not describe the swap that was asked for.
    #[error("the swap quote {detail}")]
    Quote { detail: String },
    /// A transfer is authorized by a wallet other than the expected one.
    #[error("transfer is authorized by {actual}, not {expected}")]
    WrongAuthority { expected: Pubkey, actual: Pubkey },
    /// A transfer moves funds out of an account other than the expected one.
    #[error("transfer moves funds from {actual}, not {expected}")]
    WrongSource { expected: Pubkey, actual: Pubkey },
    /// The amounts or recipients do not match what was asked for.
    #[error("the transfers do not match what was requested")]
    TransfersDiffer,
    /// The transaction destroys a different amount than was asked for.
    #[error("the transaction burns {actual}, not the requested {expected}")]
    BurnDiffers { expected: u64, actual: u64 },
    /// Instruction data is too short to read a field the check needs.
    #[error("{what} could not be read")]
    Malformed { what: &'static str },
}

/// The program a compiled instruction invokes.
///
/// Fails when the index points past the static keys. Solana rejects such a
/// transaction anyway, so this cannot be reached on chain; it is here so a
/// caller walking instructions never silently skips one it could not read.
pub fn instruction_program(
    tx: &VersionedTransaction,
    ix: &CompiledInstruction,
) -> Result<Pubkey, VerifyError> {
    tx.message
        .static_account_keys()
        .get(ix.program_id_index as usize)
        .copied()
        .ok_or(VerifyError::LookupTable { what: "a program" })
}

/// An account an instruction names, by its position in that instruction.
///
/// Fails when the account resolves through a lookup table, for the same reason
/// as [`instruction_program`]: an unread account is not a checked one.
pub fn instruction_account(
    tx: &VersionedTransaction,
    ix: &CompiledInstruction,
    slot: usize,
) -> Result<Pubkey, VerifyError> {
    let index = *ix
        .accounts
        .get(slot)
        .ok_or(VerifyError::LookupTable { what: "an account" })? as usize;
    tx.message
        .static_account_keys()
        .get(index)
        .copied()
        .ok_or(VerifyError::LookupTable { what: "an account" })
}

/// Worst-case prioritization fee in lamports.
///
/// Solana charges `limit x price / 1e6`, rounded up. An absent limit means the
/// runtime default, which the fee is still charged against, so it is priced
/// too.
fn max_prioritization_fee(tx: &VersionedTransaction) -> u64 {
    let compute_budget = KnownProgram::ComputeBudget.id();
    let keys = tx.message.static_account_keys();
    let (mut price, mut limit) = (0u64, None);
    for ix in tx.message.instructions() {
        if keys.get(ix.program_id_index as usize) != Some(&compute_budget) {
            continue;
        }
        match ix.data.split_first() {
            // SetComputeUnitLimit(u32)
            Some((2, rest)) if rest.len() >= 4 => {
                limit = Some(u64::from(u32::from_le_bytes(
                    rest[..4].try_into().expect("4 bytes"),
                )));
            }
            // SetComputeUnitPrice(u64)
            Some((3, rest)) if rest.len() >= 8 => {
                price = u64::from_le_bytes(rest[..8].try_into().expect("8 bytes"));
            }
            _ => {}
        }
    }
    let limit = limit.unwrap_or_else(|| {
        (tx.message.instructions().len() as u64 * DEFAULT_COMPUTE_UNITS_PER_IX)
            .min(MAX_COMPUTE_UNIT_LIMIT)
    });
    limit.saturating_mul(price).div_ceil(1_000_000)
}

/// The single transaction a builder returned, held to what any caller is
/// willing to authorize before its contents are looked at.
///
/// One transaction, because an action expected to need one that arrives as
/// several is not that action. Priced within `max_micro_lamports`, because the
/// compute-budget instructions carry no intent a reviewer can read. And
/// requiring only `sole_signer`'s signature when one is named -- `None` allows
/// a co-signature, which some actions legitimately carry.
///
/// This is the preamble to every per-action check; those say what the
/// transaction must *do*, this says what shape it has to arrive in.
pub fn sole_signable(
    data: &crate::blockchain_api::types::TransactionData,
    sole_signer: Option<&Pubkey>,
    max_micro_lamports: u64,
) -> Result<VersionedTransaction, VerifyError> {
    let mut txns = data
        .decode_transactions()
        .map_err(|_| VerifyError::Malformed {
            what: "a returned transaction",
        })?;
    if txns.len() != 1 {
        return Err(VerifyError::TransactionCount { found: txns.len() });
    }
    let txn = txns.remove(0);
    assert_priority_fee_within(&txn, max_micro_lamports)?;
    if let Some(signer) = sole_signer {
        assert_sole_signer(&txn, signer)?;
    }
    Ok(txn)
}

/// Refuse a transaction whose compute-unit price exceeds `max_micro_lamports`.
///
/// The compute-budget program carries no intent a reviewer can read, so a
/// ceiling is the only useful control over what a transaction costs in SOL.
pub fn assert_priority_fee_within(
    tx: &VersionedTransaction,
    max_micro_lamports: u64,
) -> Result<u64, VerifyError> {
    let lamports = max_prioritization_fee(tx);
    let cap = max_micro_lamports
        .saturating_mul(MAX_COMPUTE_UNIT_LIMIT)
        .div_ceil(1_000_000);
    if lamports > cap {
        return Err(VerifyError::PriorityFee { lamports, cap });
    }
    Ok(lamports)
}

/// Refuse unless `wallet` is the transaction's only required signer.
///
/// The sole required signer is account 0, so this also establishes that the
/// wallet pays the fee.
pub fn assert_sole_signer(tx: &VersionedTransaction, wallet: &Pubkey) -> Result<(), VerifyError> {
    let required = tx.message.header().num_required_signatures;
    if required != 1 {
        return Err(VerifyError::SignerCount { required });
    }
    match tx.message.static_account_keys().first() {
        Some(signer) if signer == wallet => Ok(()),
        actual => Err(VerifyError::WrongSigner {
            expected: *wallet,
            actual: actual.copied(),
        }),
    }
}

/// A top-level instruction [`find_methods`] matched, with the transaction it
/// came from so its accounts can be resolved.
#[derive(Debug)]
pub struct NamedInstruction<'a> {
    tx: &'a VersionedTransaction,
    instruction: &'a CompiledInstruction,
    program: KnownProgram,
    /// The Anchor method name, for naming the action in a refusal.
    pub method: &'static str,
}

impl NamedInstruction<'_> {
    /// The account this instruction passes in position `index`.
    pub fn account(&self, index: usize) -> Result<Pubkey, VerifyError> {
        instruction_account(self.tx, self.instruction, index)
    }

    /// This instruction's arguments, decoded against the program's IDL.
    ///
    /// `None` when the body does not decode, which a caller checking an
    /// argument has to refuse on: an unreadable argument is not an absent one.
    pub fn args(&self) -> Option<serde_json::Value> {
        let discriminator: [u8; 8] = self.instruction.data.get(..8)?.try_into().ok()?;
        self.program
            .decode_instruction_args(&discriminator, &self.instruction.data[8..])
    }
}

/// Every top-level instruction across `txs` that invokes `program` by one of
/// `methods`.
///
/// A caller checks the accounts of what this returns, so what it does not
/// return is as load-bearing as what it does: an instruction whose program has
/// no shipped IDL, or whose discriminator that IDL does not name, is not a
/// match. Pair it with an emptiness check, or an action that was never built
/// reads the same as one that was built correctly.
pub fn find_methods<'a>(
    txs: &'a [VersionedTransaction],
    program: KnownProgram,
    methods: &[&str],
) -> Result<Vec<NamedInstruction<'a>>, VerifyError> {
    let id = program.id();
    let mut found = Vec::new();
    for tx in txs {
        for instruction in tx.message.instructions() {
            if instruction_program(tx, instruction)? != id {
                continue;
            }
            let Some(bytes) = instruction.data.get(..8) else {
                continue;
            };
            let discriminator: [u8; 8] = bytes.try_into().expect("8 bytes");
            let Some(method) = program.method_name(&discriminator) else {
                continue;
            };
            if methods.contains(&method) {
                found.push(NamedInstruction {
                    tx,
                    instruction,
                    program,
                    method,
                });
            }
        }
    }
    Ok(found)
}

/// Jupiter aggregator v6, the program that executes a swap route. Taken from a
/// transaction the blockchain-api built for HNT->USDC, which invokes exactly
/// this and the compute-budget program.
pub const JUPITER_AGGREGATOR_V6: Pubkey = pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4");

/// SPL-token instruction tags a swap may legitimately carry. Wrapping and
/// unwrapping SOL is the only reason one appears at the top level, so the tags
/// that move or delegate a balance are absent by design.
const SWAP_ALLOWED_SPL_TAGS: &[u8] = &[
    9,  // CloseAccount, unwrapping the temporary wSOL account
    17, // SyncNative
    18, // InitializeAccount3
];

/// Refuse a swap that carries anything but the swap.
///
/// A signature check admits every transaction this wallet's key can authorize,
/// which is each token transfer, delegation and authority change it owns. What
/// makes a route safe to sign is that it invokes only the aggregator and the
/// account plumbing a swap needs, so that is checked rather than inferred from
/// the endpoint that built it.
pub fn assert_swap_only(txs: &[VersionedTransaction]) -> Result<(), VerifyError> {
    let compute_budget = KnownProgram::ComputeBudget.id();
    let spl_token = KnownProgram::SplToken.id();
    let associated_token = KnownProgram::SplAssociatedToken.id();
    for tx in txs {
        for ix in tx.message.instructions() {
            let program = instruction_program(tx, ix)?;
            if program == JUPITER_AGGREGATOR_V6
                || program == associated_token
                || program == compute_budget
            {
                continue;
            }
            if program == spl_token {
                let tag = ix.data.first().copied().unwrap_or(u8::MAX);
                if !SWAP_ALLOWED_SPL_TAGS.contains(&tag) {
                    return Err(VerifyError::UnexpectedInstruction {
                        program,
                        tag: ix.data.first().copied(),
                    });
                }
                continue;
            }
            return Err(VerifyError::UnexpectedProgram { program });
        }
    }
    Ok(())
}

/// Confirm a quote describes the swap that was asked for, before it is sent
/// back to be built into a transaction.
///
/// `other_amount_threshold` becomes the route's on-chain minimum output, so it
/// is the only thing holding execution to a price. Left unchecked, a quote
/// reporting a healthy `price_impact_pct` can still carry a threshold of 1.
pub fn assert_quote_matches(
    quote: &SwapQuote,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
    amount: u64,
    max_slippage_bps: u16,
) -> Result<(), VerifyError> {
    let mismatch = |detail: String| Err(VerifyError::Quote { detail });
    if quote.input_mint != input_mint.to_string() || quote.output_mint != output_mint.to_string() {
        return mismatch(format!(
            "is for {} -> {}, not {input_mint} -> {output_mint}",
            quote.input_mint, quote.output_mint
        ));
    }
    if quote.swap_mode != "ExactIn" {
        return mismatch(format!("swap mode is {}, not ExactIn", quote.swap_mode));
    }
    let parse = |what: &str, raw: &str| {
        raw.parse::<u64>().map_err(|_| VerifyError::Quote {
            detail: format!("{what} {raw} is not a u64"),
        })
    };
    let in_amount = parse("in_amount", &quote.in_amount)?;
    if in_amount != amount {
        return mismatch(format!("is for {in_amount}, not the {amount} requested"));
    }
    // Above the full 10000 bps there is no output floor to imply, and the
    // subtraction below would wrap.
    if max_slippage_bps > 10_000 {
        return mismatch(format!(
            "cannot be held to {max_slippage_bps} bps, which is more than the whole output"
        ));
    }
    if quote.slippage_bps > u32::from(max_slippage_bps) {
        return mismatch(format!(
            "slippage {} bps exceeds the {max_slippage_bps} bps requested",
            quote.slippage_bps
        ));
    }
    let out = parse("out_amount", &quote.out_amount)?;
    let threshold = parse("threshold", &quote.other_amount_threshold)?;
    let floor = out
        .saturating_mul(u64::from(10_000 - max_slippage_bps))
        .div_ceil(10_000);
    if threshold < floor {
        return mismatch(format!(
            "sets a minimum output of {threshold}, below the {floor} implied by \
             {max_slippage_bps} bps on an output of {out}"
        ));
    }
    Ok(())
}

/// SPL-token instruction tags that destroy a balance: `Burn` and `BurnChecked`.
/// Both lay out `[account, mint, authority]`, so only the trailing decimals byte
/// differs and the amount sits in the same place.
const SPL_BURN_TAGS: [u8; 2] = [8, 15];

/// Refuse unless the transaction burns exactly `amount` of `token` from
/// `wallet`'s associated account, and does nothing else that moves a balance.
///
/// A burn is not recoverable, so the amount and the account it comes out of are
/// the whole of what a signer is agreeing to.
pub fn assert_spl_burn(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    token: crate::token::Token,
    amount: u64,
) -> Result<(), VerifyError> {
    let spl_token = KnownProgram::SplToken.id();
    let compute_budget = KnownProgram::ComputeBudget.id();
    let source = token.associated_token_address(wallet);
    let mut burned: u64 = 0;
    let mut seen = 0usize;

    for tx in unsigned {
        for ix in tx.message.instructions() {
            let program = instruction_program(tx, ix)?;
            if program == compute_budget {
                continue;
            }
            if program != spl_token {
                return Err(VerifyError::UnexpectedProgram { program });
            }
            let tag = ix.data.first().copied().unwrap_or(u8::MAX);
            if !SPL_BURN_TAGS.contains(&tag) {
                return Err(VerifyError::UnexpectedInstruction {
                    program,
                    tag: ix.data.first().copied(),
                });
            }
            let from = instruction_account(tx, ix, 0)?;
            if from != source {
                return Err(VerifyError::WrongSource {
                    expected: source,
                    actual: from,
                });
            }
            let authority = instruction_account(tx, ix, 2)?;
            if authority != *wallet {
                return Err(VerifyError::WrongAuthority {
                    expected: *wallet,
                    actual: authority,
                });
            }
            burned = burned.saturating_add(spl_transfer_amount(&ix.data)?);
            seen += 1;
        }
    }

    // `seen == 0` is its own case: a response carrying no burn at all would
    // otherwise pass whenever the requested amount happened to be zero.
    if seen == 0 || burned != amount {
        return Err(VerifyError::BurnDiffers {
            expected: amount,
            actual: burned,
        });
    }
    Ok(())
}

/// SPL-token instruction tags that move a balance: `Transfer` and
/// `TransferChecked`. Their account layouts differ, which is why the tag has to
/// be read before the accounts mean anything.
const SPL_TRANSFER_TAGS: [u8; 2] = [3, 12];

/// The raw amount an SPL transfer moves, from its instruction data.
fn spl_transfer_amount(data: &[u8]) -> Result<u64, VerifyError> {
    data.get(1..9)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(VerifyError::Malformed {
            what: "an SPL-token transfer amount",
        })
}

/// Refuse a transaction unless it moves exactly `expected` of `token` out of
/// `wallet`, and does nothing else.
///
/// `expected` is (recipient, raw amount) pairs; a recipient listed twice is
/// summed, so the comparison is against totals rather than instruction order.
///
/// The accounts that pin the outcome are the wallet's own token account as the
/// source, the wallet as the authority, and each recipient's token account as
/// the destination. Those are wallet-specific and are never packed into the
/// shared lookup table, so they are always static keys. Requiring the source to
/// be the wallet's `token` account fixes the mint without reading a mint
/// account that might itself be table-loaded.
pub fn assert_spl_transfers(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    token: crate::token::Token,
    expected: &[(Pubkey, u64)],
) -> Result<(), VerifyError> {
    let spl_token = KnownProgram::SplToken.id();
    let compute_budget = KnownProgram::ComputeBudget.id();
    let associated_token = KnownProgram::SplAssociatedToken.id();
    let source_account = token.associated_token_address(wallet);

    let mut want: HashMap<Pubkey, u64> = HashMap::new();
    for (recipient, amount) in expected {
        *want
            .entry(token.associated_token_address(recipient))
            .or_default() += amount;
    }

    let mut got: HashMap<Pubkey, u64> = HashMap::new();
    for tx in unsigned {
        for ix in tx.message.instructions() {
            let program = instruction_program(tx, ix)?;
            if program == compute_budget {
                continue;
            }
            if program == associated_token {
                // Tag 1 is CreateIdempotent, the only reason a transfer creates
                // an account. Tag 0 funds a brand-new account from the signer at
                // rent cost, for any owner, which the amount totals do not see.
                match ix.data.first() {
                    Some(1) => continue,
                    tag => {
                        return Err(VerifyError::UnexpectedInstruction {
                            program,
                            tag: tag.copied(),
                        })
                    }
                }
            }
            if program != spl_token {
                return Err(VerifyError::UnexpectedProgram { program });
            }
            let tag = ix.data.first().copied().unwrap_or(u8::MAX);
            if !SPL_TRANSFER_TAGS.contains(&tag) {
                return Err(VerifyError::UnexpectedInstruction {
                    program,
                    tag: ix.data.first().copied(),
                });
            }
            // Transfer is [source, dest, authority]; TransferChecked inserts the
            // mint, giving [source, mint, dest, authority].
            let (source, dest, authority) = if tag == 12 {
                (
                    instruction_account(tx, ix, 0)?,
                    instruction_account(tx, ix, 2)?,
                    instruction_account(tx, ix, 3)?,
                )
            } else {
                (
                    instruction_account(tx, ix, 0)?,
                    instruction_account(tx, ix, 1)?,
                    instruction_account(tx, ix, 2)?,
                )
            };
            if authority != *wallet {
                return Err(VerifyError::WrongAuthority {
                    expected: *wallet,
                    actual: authority,
                });
            }
            if source != source_account {
                return Err(VerifyError::WrongSource {
                    expected: source_account,
                    actual: source,
                });
            }
            *got.entry(dest).or_default() += spl_transfer_amount(&ix.data)?;
        }
    }

    if got != want {
        return Err(VerifyError::TransfersDiffer);
    }
    Ok(())
}

/// Refuse a transaction anchored to a durable nonce.
///
/// A durable nonce must be advanced by the first instruction, and anchoring to
/// one removes the blockhash expiry, so the signature stays submittable
/// indefinitely.
pub fn assert_not_nonce_anchored(tx: &VersionedTransaction) -> Result<(), VerifyError> {
    let Some(ix) = tx.message.instructions().first() else {
        return Ok(());
    };
    let program = instruction_program(tx, ix)?;
    // System-program tags are little-endian u32; 4 is AdvanceNonceAccount.
    if program == KnownProgram::SystemProgram.id() && ix.data.starts_with(&[4, 0, 0, 0]) {
        return Err(VerifyError::NonceAnchored);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solana_sdk::message::{Message, MessageHeader, VersionedMessage};

    /// A transaction whose first account is `signer`, carrying `(program, data)`
    /// pairs as instructions.
    fn tx(signer: Pubkey, required: u8, ixs: &[(Pubkey, Vec<u8>)]) -> VersionedTransaction {
        let mut keys = vec![signer];
        let instructions = ixs
            .iter()
            .map(|(program, data)| {
                keys.push(*program);
                CompiledInstruction {
                    program_id_index: (keys.len() - 1) as u8,
                    accounts: vec![0],
                    data: data.clone(),
                }
            })
            .collect();
        VersionedTransaction {
            signatures: vec![Default::default(); required.max(1) as usize],
            message: VersionedMessage::Legacy(Message {
                header: MessageHeader {
                    num_required_signatures: required,
                    num_readonly_signed_accounts: 0,
                    num_readonly_unsigned_accounts: 0,
                },
                account_keys: keys,
                recent_blockhash: Default::default(),
                instructions,
            }),
        }
    }

    fn budget(limit: u32, price: u64) -> Vec<(Pubkey, Vec<u8>)> {
        let cb = KnownProgram::ComputeBudget.id();
        let mut l = vec![2u8];
        l.extend_from_slice(&limit.to_le_bytes());
        let mut p = vec![3u8];
        p.extend_from_slice(&price.to_le_bytes());
        vec![(cb, l), (cb, p)]
    }

    #[test]
    fn prioritization_fee_is_limit_times_price() {
        // 1.4M CU at 1000 micro-lamports/CU is 1400 lamports.
        assert_eq!(
            max_prioritization_fee(&tx(Pubkey::new_unique(), 1, &budget(1_400_000, 1_000))),
            1_400
        );
    }

    #[test]
    fn an_absent_price_costs_nothing() {
        let cb = KnownProgram::ComputeBudget.id();
        let mut l = vec![2u8];
        l.extend_from_slice(&1_400_000u32.to_le_bytes());
        assert_eq!(
            max_prioritization_fee(&tx(Pubkey::new_unique(), 1, &[(cb, l)])),
            0
        );
    }

    #[test]
    fn an_absent_limit_is_priced_at_the_runtime_default() {
        let cb = KnownProgram::ComputeBudget.id();
        let mut p = vec![3u8];
        p.extend_from_slice(&1_000_000u64.to_le_bytes());
        // One instruction defaults to 200k CU, at 1 lamport per CU.
        assert_eq!(
            max_prioritization_fee(&tx(Pubkey::new_unique(), 1, &[(cb, p)])),
            200_000
        );
    }

    #[test]
    fn a_price_above_the_cap_is_refused() {
        // Sized to take roughly a whole SOL in priority fee.
        let t = tx(Pubkey::new_unique(), 1, &budget(1_400_000, 714_285_714));
        let err = assert_priority_fee_within(&t, 2_500_000).expect_err("must refuse");
        assert!(matches!(err, VerifyError::PriorityFee { .. }), "{err}");
    }

    #[test]
    fn a_price_at_the_cap_is_allowed() {
        let t = tx(Pubkey::new_unique(), 1, &budget(1_400_000, 2_500_000));
        // 1.4M CU at 2.5M micro-lamports/CU is 3,500,000 lamports, or 0.0035
        // SOL: the ceiling itself, which must be allowed rather than refused.
        assert_eq!(
            assert_priority_fee_within(&t, 2_500_000).expect("the cap itself must pass"),
            3_500_000
        );
    }

    #[test]
    fn sole_signer_accepts_only_that_wallet() {
        let wallet = Pubkey::new_unique();
        assert_sole_signer(&tx(wallet, 1, &[]), &wallet).expect("the wallet signs");
        let err = assert_sole_signer(&tx(wallet, 1, &[]), &Pubkey::new_unique())
            .expect_err("another wallet must be refused");
        assert!(matches!(err, VerifyError::WrongSigner { .. }), "{err}");
    }

    #[test]
    fn sole_signer_refuses_a_co_signed_transaction() {
        let wallet = Pubkey::new_unique();
        let err = assert_sole_signer(&tx(wallet, 2, &[]), &wallet)
            .expect_err("two required signers must be refused");
        assert!(
            matches!(err, VerifyError::SignerCount { required: 2 }),
            "{err}"
        );
    }

    /// `sha256("global:mint_data_credits_v0")[..8]`, the discriminator the
    /// data-credits IDL declares for that method.
    const MINT_DC: [u8; 8] = [78, 109, 169, 132, 144, 94, 221, 57];

    #[test]
    fn a_named_method_on_the_named_program_is_found() {
        let dc = KnownProgram::DataCredits.id();
        let t = tx(Pubkey::new_unique(), 1, &[(dc, MINT_DC.to_vec())]);
        let found = find_methods(
            std::slice::from_ref(&t),
            KnownProgram::DataCredits,
            &["mint_data_credits_v0"],
        )
        .expect("a readable transaction");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].method, "mint_data_credits_v0");
    }

    #[test]
    fn the_same_discriminator_on_a_different_program_is_not_a_match() {
        // Anchor discriminators are per-method, not per-program, so a matching
        // 8 bytes on another program must not answer for the one asked about.
        let other = KnownProgram::Fanout.id();
        let t = tx(Pubkey::new_unique(), 1, &[(other, MINT_DC.to_vec())]);
        let found = find_methods(
            std::slice::from_ref(&t),
            KnownProgram::DataCredits,
            &["mint_data_credits_v0"],
        )
        .expect("a readable transaction");
        assert!(
            found.is_empty(),
            "matched {} on the wrong program",
            found.len()
        );
    }

    #[test]
    fn a_method_outside_the_requested_set_is_not_a_match() {
        let dc = KnownProgram::DataCredits.id();
        let t = tx(Pubkey::new_unique(), 1, &[(dc, MINT_DC.to_vec())]);
        let found = find_methods(
            std::slice::from_ref(&t),
            KnownProgram::DataCredits,
            &["burn_without_tracking_v0"],
        )
        .expect("a readable transaction");
        assert!(found.is_empty());
    }

    #[test]
    fn an_instruction_too_short_to_carry_a_discriminator_is_not_a_match() {
        let dc = KnownProgram::DataCredits.id();
        let t = tx(Pubkey::new_unique(), 1, &[(dc, vec![78, 109, 169])]);
        let found = find_methods(
            std::slice::from_ref(&t),
            KnownProgram::DataCredits,
            &["mint_data_credits_v0"],
        )
        .expect("a readable transaction");
        assert!(found.is_empty());
    }

    #[test]
    fn a_program_loaded_from_a_lookup_table_is_refused_rather_than_skipped() {
        // Skipping it would let an instruction the caller cannot read pass as
        // an instruction that is not there.
        let dc = KnownProgram::DataCredits.id();
        let mut t = tx(Pubkey::new_unique(), 1, &[(dc, MINT_DC.to_vec())]);
        if let VersionedMessage::Legacy(msg) = &mut t.message {
            msg.instructions[0].program_id_index = 99;
        }
        let err = find_methods(
            std::slice::from_ref(&t),
            KnownProgram::DataCredits,
            &["mint_data_credits_v0"],
        )
        .expect_err("an unreadable program must be refused");
        assert!(matches!(err, VerifyError::LookupTable { .. }), "{err}");
    }

    fn quote(over: &[(&str, &str)]) -> SwapQuote {
        let mut q = SwapQuote {
            input_mint: "So11111111111111111111111111111111111111112".into(),
            output_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
            in_amount: "1000".into(),
            out_amount: "2000".into(),
            other_amount_threshold: "1980".into(),
            swap_mode: "ExactIn".into(),
            slippage_bps: 100,
            price_impact_pct: "0.01".into(),
            extra: serde_json::Value::Null,
        };
        for (field, value) in over {
            match *field {
                "input_mint" => q.input_mint = (*value).into(),
                "output_mint" => q.output_mint = (*value).into(),
                "in_amount" => q.in_amount = (*value).into(),
                "out_amount" => q.out_amount = (*value).into(),
                "other_amount_threshold" => q.other_amount_threshold = (*value).into(),
                "swap_mode" => q.swap_mode = (*value).into(),
                "slippage_bps" => q.slippage_bps = value.parse().expect("a bps number"),
                other => panic!("unknown field {other}"),
            }
        }
        q
    }

    fn mints() -> (Pubkey, Pubkey) {
        (
            "So11111111111111111111111111111111111111112"
                .parse()
                .expect("the wSOL mint"),
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
                .parse()
                .expect("the USDC mint"),
        )
    }

    fn check_quote(q: &SwapQuote, slippage: u16) -> Result<(), VerifyError> {
        let (input, output) = mints();
        assert_quote_matches(q, &input, &output, 1000, slippage)
    }

    #[test]
    fn a_quote_for_the_requested_swap_is_accepted() {
        check_quote(&quote(&[]), 100).expect("the requested quote");
    }

    #[test]
    fn a_quote_for_other_mints_is_refused() {
        let err = check_quote(
            &quote(&[("output_mint", "11111111111111111111111111111111")]),
            100,
        )
        .expect_err("a substituted output mint must be refused");
        assert!(err.to_string().contains("not"), "{err}");
    }

    #[test]
    fn a_quote_for_a_different_amount_is_refused() {
        let err = check_quote(&quote(&[("in_amount", "999")]), 100)
            .expect_err("a substituted input amount must be refused");
        assert!(err.to_string().contains("999"), "{err}");
    }

    #[test]
    fn a_quote_whose_minimum_output_is_below_the_slippage_floor_is_refused() {
        // The field that actually holds execution to a price: a healthy
        // price_impact_pct alongside a threshold of 1 is the whole attack.
        let err = check_quote(&quote(&[("other_amount_threshold", "1")]), 100)
            .expect_err("a floorless quote must be refused");
        assert!(err.to_string().contains("minimum output"), "{err}");
        check_quote(&quote(&[("other_amount_threshold", "1980")]), 100)
            .expect("exactly the floor is allowed");
        // Both sides of the boundary, or an off-by-one in the comparison keeps
        // the "exactly the floor" case passing and gives away a basis point.
        check_quote(&quote(&[("other_amount_threshold", "1979")]), 100)
            .expect_err("one below the floor must be refused");
    }

    #[test]
    fn a_quote_slippier_than_requested_is_refused() {
        let err = check_quote(&quote(&[("slippage_bps", "300")]), 100)
            .expect_err("a widened slippage must be refused");
        assert!(err.to_string().contains("300"), "{err}");
    }

    #[test]
    fn an_exact_out_quote_is_refused() {
        let err = check_quote(&quote(&[("swap_mode", "ExactOut")]), 100)
            .expect_err("a mode that does not spend the stated input must be refused");
        assert!(err.to_string().contains("ExactIn"), "{err}");
    }

    #[test]
    fn a_slippage_wider_than_the_whole_output_is_refused_rather_than_wrapping() {
        let err =
            check_quote(&quote(&[]), 20_000).expect_err("an impossible slippage must be refused");
        assert!(err.to_string().contains("whole output"), "{err}");
    }

    /// An SPL `Burn` (tag 8) laid out `[account, mint, authority]`.
    fn burn_ix(source: Pubkey, authority: Pubkey, amount: u64) -> VersionedTransaction {
        let mut data = vec![8u8];
        data.extend_from_slice(&amount.to_le_bytes());
        let keys = vec![
            authority,
            KnownProgram::SplToken.id(),
            source,
            Pubkey::new_unique(),
        ];
        VersionedTransaction {
            signatures: vec![Default::default()],
            message: VersionedMessage::Legacy(Message {
                header: MessageHeader {
                    num_required_signatures: 1,
                    num_readonly_signed_accounts: 0,
                    num_readonly_unsigned_accounts: 0,
                },
                account_keys: keys,
                recent_blockhash: Default::default(),
                instructions: vec![CompiledInstruction {
                    program_id_index: 1,
                    // [source, mint, authority]
                    accounts: vec![2, 3, 0],
                    data,
                }],
            }),
        }
    }

    #[test]
    fn a_burn_of_the_requested_amount_is_accepted() {
        let wallet = Pubkey::new_unique();
        let source = crate::token::Token::Hnt.associated_token_address(&wallet);
        assert_spl_burn(
            std::slice::from_ref(&burn_ix(source, wallet, 500)),
            &wallet,
            crate::token::Token::Hnt,
            500,
        )
        .expect("the requested burn");
    }

    #[test]
    fn a_burn_of_a_different_amount_is_refused() {
        let wallet = Pubkey::new_unique();
        let source = crate::token::Token::Hnt.associated_token_address(&wallet);
        let err = assert_spl_burn(
            std::slice::from_ref(&burn_ix(source, wallet, 501)),
            &wallet,
            crate::token::Token::Hnt,
            500,
        )
        .expect_err("a substituted amount must be refused");
        assert!(
            matches!(
                err,
                VerifyError::BurnDiffers {
                    expected: 500,
                    actual: 501
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn a_burn_from_another_account_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_spl_burn(
            std::slice::from_ref(&burn_ix(Pubkey::new_unique(), wallet, 500)),
            &wallet,
            crate::token::Token::Hnt,
            500,
        )
        .expect_err("a burn from another account must be refused");
        assert!(matches!(err, VerifyError::WrongSource { .. }), "{err}");
    }

    #[test]
    fn a_response_carrying_no_burn_is_refused_even_for_zero() {
        // Zero is the case an emptiness check has to be separate for: without
        // it, a response carrying nothing would satisfy a burn of nothing.
        let wallet = Pubkey::new_unique();
        let err = assert_spl_burn(&[], &wallet, crate::token::Token::Hnt, 0)
            .expect_err("a response with no burn must be refused");
        assert!(matches!(err, VerifyError::BurnDiffers { .. }), "{err}");
    }

    #[test]
    fn a_transfer_bundled_with_a_burn_is_refused() {
        let wallet = Pubkey::new_unique();
        let source = crate::token::Token::Hnt.associated_token_address(&wallet);
        let mut t = burn_ix(source, wallet, 500);
        if let VersionedMessage::Legacy(msg) = &mut t.message {
            msg.instructions[0].data[0] = 3; // Transfer
        }
        let err = assert_spl_burn(
            std::slice::from_ref(&t),
            &wallet,
            crate::token::Token::Hnt,
            500,
        )
        .expect_err("a transfer must not pass as a burn");
        assert!(
            matches!(err, VerifyError::UnexpectedInstruction { tag: Some(3), .. }),
            "{err}"
        );
    }

    #[test]
    fn a_swap_invoking_only_the_aggregator_is_accepted() {
        let payer = Pubkey::new_unique();
        let t = tx(
            payer,
            1,
            &[
                (KnownProgram::ComputeBudget.id(), vec![2, 0, 0, 0, 0]),
                (JUPITER_AGGREGATOR_V6, vec![1, 2, 3]),
                (KnownProgram::SplToken.id(), vec![9]),
            ],
        );
        assert_swap_only(std::slice::from_ref(&t)).expect("a plain swap route");
    }

    #[test]
    fn a_swap_carrying_a_token_transfer_is_refused() {
        let t = tx(
            Pubkey::new_unique(),
            1,
            &[(KnownProgram::SplToken.id(), vec![3, 0, 0, 0, 0, 0, 0, 0, 0])],
        );
        let err = assert_swap_only(std::slice::from_ref(&t))
            .expect_err("an SPL transfer inside a swap must be refused");
        // The tag, not just the program: a refusal an operator reads without
        // the transaction in front of them has to name what was refused.
        assert!(
            matches!(err, VerifyError::UnexpectedInstruction { tag: Some(3), .. }),
            "{err}"
        );
    }

    #[test]
    fn a_swap_invoking_an_unrelated_program_is_refused() {
        let other = Pubkey::new_unique();
        let t = tx(Pubkey::new_unique(), 1, &[(other, vec![0])]);
        let err = assert_swap_only(std::slice::from_ref(&t))
            .expect_err("a program outside a swap must be refused");
        assert!(
            matches!(err, VerifyError::UnexpectedProgram { program } if program == other),
            "{err}"
        );
    }

    #[test]
    fn a_lookup_table_program_is_refused() {
        let mut t = tx(Pubkey::new_unique(), 1, &[(Pubkey::new_unique(), vec![0])]);
        if let VersionedMessage::Legacy(msg) = &mut t.message {
            msg.instructions[0].program_id_index = 99;
        }
        let ix = &t.message.instructions()[0];
        let err = instruction_program(&t, ix).expect_err("an unreadable program is refused");
        assert!(matches!(err, VerifyError::LookupTable { .. }), "{err}");
    }

    #[test]
    fn a_nonce_anchored_transaction_is_refused() {
        let system = KnownProgram::SystemProgram.id();
        // Tag 4 is AdvanceNonceAccount; tag 2 is an ordinary Transfer.
        let anchored = tx(Pubkey::new_unique(), 1, &[(system, vec![4, 0, 0, 0])]);
        assert!(matches!(
            assert_not_nonce_anchored(&anchored).expect_err("must refuse"),
            VerifyError::NonceAnchored
        ));
        let ordinary = tx(Pubkey::new_unique(), 1, &[(system, vec![2, 0, 0, 0])]);
        assert_not_nonce_anchored(&ordinary).expect("an ordinary transfer is fine");
    }
}
