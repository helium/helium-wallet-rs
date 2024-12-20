use crate::{
    client::SolanaRpcClient,
    error::Error,
    keypair::{Keypair, Pubkey},
    mk_transaction_with_blockhash, priority_fee,
    solana_client::rpc_client::SerializableTransaction,
    solana_sdk::{signer::Signer, transaction::Transaction},
    TransactionOpts,
};

pub async fn memo_transaction<C: AsRef<SolanaRpcClient>>(
    client: &C,
    data: &str,
    pubkey: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(Transaction, u64), Error> {
    let ix = spl_memo::build_memo(data.as_bytes(), &[pubkey]);
    let ixs = &[
        priority_fee::compute_budget_instruction(200_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &ix.accounts,
            opts.min_priority_fee,
        )
        .await?,
        ix,
    ];

    mk_transaction_with_blockhash(client, ixs, pubkey).await
}

pub async fn memo<C: AsRef<SolanaRpcClient>>(
    client: &C,
    data: &str,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(Transaction, u64), Error> {
    let (mut txn, latest_block_height) =
        memo_transaction(client, data, &keypair.pubkey(), opts).await?;
    txn.try_sign(&[keypair], *txn.get_recent_blockhash())?;
    Ok((txn, latest_block_height))
}
