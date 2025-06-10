use crate::{
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    error::Error,
    keypair::Keypair,
    message, priority_fee,
    programs::hpl_crons,
    solana_sdk::instruction::Instruction,
    transaction::{mk_transaction, VersionedTransaction},
    Pubkey, TransactionOpts,
};
use anchor_lang::{InstructionData, ToAccountMetas};
use solana_sdk::signer::Signer;
use tuktuk_sdk::tuktuk_program::TaskQueueV0;
use tuktuk_sdk::{tuktuk, tuktuk_program};

pub fn claim_wallet_key(task_queue_key: &Pubkey, wallet: &Pubkey) -> Pubkey {
    tuktuk::custom_signer_key(task_queue_key, &[b"claim_payer", wallet.as_ref()])
}

pub fn claim_wallet_instruction(
    task_queue_key: &Pubkey,
    task_queue: &TaskQueueV0,
    wallet: &Pubkey,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    fn mk_accounts(
        task_queue_key: &Pubkey,
        task_id: u16,
        wallet: &Pubkey,
        payer: &Pubkey,
    ) -> impl ToAccountMetas {
        let queue_authority = tuktuk::task_queue::queue_authority_key(&hpl_crons::ID);
        hpl_crons::client::accounts::QueueWalletClaimV0 {
            payer: *payer,
            wallet: *wallet,
            pda_wallet: claim_wallet_key(task_queue_key, wallet),
            system_program: solana_sdk::system_program::ID,
            tuktuk_program: tuktuk_program::tuktuk::ID,
            queue_authority,
            task_queue: *task_queue_key,
            task_queue_authority: tuktuk::task_queue::task_queue_authority_key(
                task_queue_key,
                &queue_authority,
            ),
            task: tuktuk::task::key(task_queue_key, task_id),
        }
    }
    let free_task_id = tuktuk::task_queue::next_available_task_ids(task_queue, 1)?[0];
    let accounts = mk_accounts(task_queue_key, free_task_id, wallet, payer);
    let ix = Instruction {
        program_id: hpl_crons::ID,
        accounts: accounts.to_account_metas(None),
        data: hpl_crons::client::args::QueueWalletClaimV0 {
            args: hpl_crons::types::QueueWalletClaimArgsV0 { free_task_id },
        }
        .data(),
    };
    Ok(ix)
}
pub async fn claim_wallet<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    task_queue_key: &Pubkey,
    wallet: &Pubkey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let task_queue = client.anchor_account(task_queue_key).await?;

    let ix = claim_wallet_instruction(task_queue_key, &task_queue, wallet, &keypair.pubkey())?;
    let ixs = &[
        priority_fee::compute_budget_instruction(100_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &ix.accounts,
            opts.fee_range(),
        )
        .await?,
        ix,
    ];

    let (msg, block_height) =
        message::mk_message(client, ixs, &opts.lut_addresses, &keypair.pubkey()).await?;
    let txn = mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
}
