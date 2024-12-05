use crate::{
    client::SolanaRpcClient,
    error::Error,
    keypair::{Keypair, Pubkey},
    priority_fee,
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
    let mut txn = Transaction::new_with_payer(ixs, Some(pubkey));
    let solana_client = AsRef::<SolanaRpcClient>::as_ref(client);
    let (latest_blockhash, latest_block_height) = solana_client
        .get_latest_blockhash_with_commitment(solana_client.commitment())
        .await?;
    txn.message.recent_blockhash = latest_blockhash;
    Ok((txn, latest_block_height))
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
