pub use solana_sdk::transaction::VersionedTransaction;

/// Builds a signed versioned transaction from a message and signers.
///
/// Requires the `txn` feature.
#[cfg(feature = "txn")]
pub fn mk_transaction<T: solana_sdk::signers::Signers + ?Sized>(
    msg: crate::message::VersionedMessage,
    signers: &T,
) -> Result<VersionedTransaction, crate::Error> {
    VersionedTransaction::try_new(msg, signers).map_err(crate::Error::from)
}

/// Signs a `(message, block_height)` pair as returned by the `*_message`
/// builders, preserving the block height for confirmation tracking.
///
/// Requires the `txn` feature.
#[cfg(feature = "txn")]
pub fn mk_signed_transaction<T: solana_sdk::signers::Signers + ?Sized>(
    (msg, block_height): (crate::message::VersionedMessage, u64),
    signers: &T,
) -> Result<(VersionedTransaction, u64), crate::Error> {
    let txn = mk_transaction(msg, signers)?;
    Ok((txn, block_height))
}
