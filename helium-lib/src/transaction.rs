use crate::{
    client::SolanaRpcClient,
    message,
    solana_sdk::{
        address_lookup_table::AddressLookupTableAccount, commitment_config::CommitmentConfig,
        instruction::Instruction, signature::Signature, signers::Signers,
    },
    Error,
};
use solana_transaction_status::TransactionConfirmationStatus;
use std::{collections::HashMap, time::Duration};

pub use solana_sdk::transaction::VersionedTransaction;
pub use solana_transaction_utils::pack::PackedTransaction;

/// Result of checking a signature's confirmation status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    /// Transaction confirmed at the requested commitment level
    Confirmed,
    /// Transaction failed on-chain (not retriable)
    Failed(String),
    /// Signature not found (may be pending or dropped)
    NotFound,
}

impl SignatureStatus {
    /// Returns true if the transaction was confirmed
    pub fn is_confirmed(&self) -> bool {
        matches!(self, SignatureStatus::Confirmed)
    }

    /// Returns true if the transaction failed on-chain
    pub fn is_failed(&self) -> bool {
        matches!(self, SignatureStatus::Failed(_))
    }

    /// Returns true if the signature was not found
    pub fn is_not_found(&self) -> bool {
        matches!(self, SignatureStatus::NotFound)
    }

    /// Convert to a ConfirmationError if not confirmed.
    /// Returns None if the status is Confirmed.
    pub fn into_error(self, signature: Signature) -> Option<crate::error::ConfirmationError> {
        match self {
            SignatureStatus::Confirmed => None,
            SignatureStatus::Failed(error) => {
                Some(crate::error::ConfirmationError::failed(signature, error))
            }
            SignatureStatus::NotFound => Some(crate::error::ConfirmationError::not_found(
                signature,
                "transaction not found on-chain",
            )),
        }
    }
}

pub fn mk_transaction<T: Signers + ?Sized>(
    msg: message::VersionedMessage,
    signers: &T,
) -> Result<VersionedTransaction, Error> {
    VersionedTransaction::try_new(msg, signers).map_err(Error::from)
}

pub fn pack_instructions(
    instructions: &[&[Instruction]],
    lookup_tables: Option<Vec<AddressLookupTableAccount>>,
) -> Result<Vec<PackedTransaction>, Error> {
    solana_transaction_utils::pack::pack_instructions_into_transactions(instructions, lookup_tables)
        .map_err(Error::from)
}

/// Check status of multiple signatures in one RPC call.
/// Returns Vec aligned with input signatures.
pub async fn get_signature_statuses<C: AsRef<SolanaRpcClient>>(
    client: &C,
    signatures: &[Signature],
    commitment: CommitmentConfig,
) -> Result<Vec<SignatureStatus>, Error> {
    let solana_client = client.as_ref();
    let response = solana_client.get_signature_statuses(signatures).await?;

    let statuses = response
        .value
        .into_iter()
        .map(|maybe_status| match maybe_status {
            None => SignatureStatus::NotFound,
            Some(status) => {
                // Check for transaction error first
                if let Some(err) = status.err {
                    return SignatureStatus::Failed(err.to_string());
                }
                // Check if confirmed at the required commitment level
                let confirmed = match commitment.commitment {
                    solana_sdk::commitment_config::CommitmentLevel::Finalized => {
                        status.confirmation_status == Some(TransactionConfirmationStatus::Finalized)
                    }
                    solana_sdk::commitment_config::CommitmentLevel::Confirmed => {
                        matches!(
                            status.confirmation_status,
                            Some(TransactionConfirmationStatus::Confirmed)
                                | Some(TransactionConfirmationStatus::Finalized)
                        )
                    }
                    _ => status.confirmations.is_some(),
                };
                if confirmed {
                    SignatureStatus::Confirmed
                } else {
                    SignatureStatus::NotFound
                }
            }
        })
        .collect();

    Ok(statuses)
}

/// Poll signatures until all confirmed or timeout.
/// Returns map of signature -> final status.
pub async fn confirm_signatures<C: AsRef<SolanaRpcClient>>(
    client: &C,
    signatures: &[Signature],
    commitment: CommitmentConfig,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<HashMap<Signature, SignatureStatus>, Error> {
    use std::time::Instant;

    let deadline = Instant::now() + timeout;
    let mut results: HashMap<Signature, SignatureStatus> = HashMap::new();
    let mut pending: Vec<Signature> = signatures.to_vec();

    while !pending.is_empty() && Instant::now() < deadline {
        let statuses = get_signature_statuses(client, &pending, commitment).await?;

        // Process results - remove confirmed/failed from pending
        let mut still_pending = Vec::new();
        for (sig, status) in pending.iter().zip(statuses.into_iter()) {
            match &status {
                SignatureStatus::Confirmed | SignatureStatus::Failed(_) => {
                    results.insert(*sig, status);
                }
                SignatureStatus::NotFound => {
                    still_pending.push(*sig);
                }
            }
        }
        pending = still_pending;

        if !pending.is_empty() && Instant::now() < deadline {
            // Use futures-timer for async sleep (compatible with any async runtime)
            futures_timer::Delay::new(poll_interval).await;
        }
    }

    // Mark any remaining as NotFound (timed out)
    for sig in pending {
        results.insert(sig, SignatureStatus::NotFound);
    }

    Ok(results)
}
