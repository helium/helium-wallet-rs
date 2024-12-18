use crate::{
    client::SolanaRpcClient,
    error::Error,
    keypair::{Keypair, Pubkey},
    mk_transaction_with_blockhash, priority_fee,
    solana_sdk::signer::Signer,
    TransactionOpts, TransactionWithBlockhash,
};

pub async fn memo_transaction<C: AsRef<SolanaRpcClient>>(
    client: &C,
    data: &str,
    pubkey: &Pubkey,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
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
) -> Result<TransactionWithBlockhash, Error> {
    let mut txn = memo_transaction(client, data, &keypair.pubkey(), opts).await?;
    txn.try_sign(&[keypair])?;
    Ok(txn)
}
