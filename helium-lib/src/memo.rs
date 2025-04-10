use crate::{
    client::SolanaRpcClient,
    error::Error,
    keypair::{Keypair, Pubkey},
    message, priority_fee,
    solana_sdk::signer::Signer,
    transaction, TransactionOpts,
};

pub async fn memo_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    data: &str,
    pubkey: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let ix = spl_memo::build_memo(data.as_bytes(), &[pubkey]);
    let ixs = &[
        priority_fee::compute_budget_instruction(200_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &ix.accounts,
            opts.fee_range(),
        )
        .await?,
        ix,
    ];

    message::mk_message(client, ixs, &opts.lut_addresses, pubkey).await
}

pub async fn memo<C: AsRef<SolanaRpcClient>>(
    client: &C,
    data: &str,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(transaction::VersionedTransaction, u64), Error> {
    let (msg, block_height) = memo_message(client, data, &keypair.pubkey(), opts).await?;
    let txn = transaction::mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
}
