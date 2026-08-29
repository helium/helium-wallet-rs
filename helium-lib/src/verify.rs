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

use crate::{
    keypair::Pubkey, programs::KnownProgram, solana_sdk::instruction::CompiledInstruction,
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
pub fn max_prioritization_fee(tx: &VersionedTransaction) -> u64 {
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

/// Refuse any top-level program outside `allowed`.
pub fn assert_programs_within(
    tx: &VersionedTransaction,
    allowed: &[Pubkey],
) -> Result<(), VerifyError> {
    for ix in tx.message.instructions() {
        let program = instruction_program(tx, ix)?;
        if !allowed.contains(&program) {
            return Err(VerifyError::UnexpectedProgram { program });
        }
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

    #[test]
    fn programs_outside_the_allowlist_are_refused() {
        let cb = KnownProgram::ComputeBudget.id();
        let other = Pubkey::new_unique();
        let t = tx(Pubkey::new_unique(), 1, &[(cb, vec![2]), (other, vec![0])]);
        assert_programs_within(&t, &[cb, other]).expect("both allowed");
        let err = assert_programs_within(&t, &[cb]).expect_err("the second must be refused");
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
        let err = assert_programs_within(&t, &[]).expect_err("an unreadable program is refused");
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
