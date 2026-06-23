use crate::{
    client::SolanaRpcClient, error::Error, keypair::Pubkey, message, solana_sdk::signer::Signer,
    transaction, TransactionOpts,
};

/// Builds a message that records an on-chain memo.
pub async fn memo_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    data: &str,
    pubkey: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let ix = spl_memo::build_memo(data.as_bytes(), &[pubkey]);
    message::mk_budgeted_message(client, 200_000, &[ix], pubkey, opts).await
}

/// Records an on-chain memo and returns a signed transaction.
pub async fn memo<C: AsRef<SolanaRpcClient>>(
    client: &C,
    data: &str,
    keypair: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(transaction::VersionedTransaction, u64), Error> {
    let msg = memo_message(client, data, &keypair.pubkey(), opts).await?;
    transaction::mk_signed_transaction(msg, &[keypair])
}
