use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    dao::Dao,
    entity_key::EncodedEntityKey,
    error::Error,
    keypair::Keypair,
    message, priority_fee,
    programs::hpl_crons::{self, accounts::CronJobV0, types::RemoveEntityFromCronArgsV0},
    queue,
    solana_sdk::{instruction::Instruction, signer::Signer, system_instruction},
    transaction::{mk_transaction, VersionedTransaction},
    tuktuk_sdk::{
        tuktuk,
        tuktuk_program::{self, TaskQueueV0},
    },
    Pubkey, TransactionOpts,
};
use itertools::Itertools;

pub fn entity_cron_authority_key(wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"entity_cron_authority", wallet.as_ref()], &hpl_crons::ID).0
}

pub fn cron_job_key_for_wallet(wallet: &Pubkey, job_id: u32) -> Pubkey {
    tuktuk::cron::cron_job_key(&entity_cron_authority_key(wallet), job_id)
}

pub async fn get<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    key: &Pubkey,
) -> Result<Option<CronJobV0>, Error> {
    match client.anchor_account(key).await {
        Ok(acc) => Ok(Some(acc)),
        Err(err) if err.is_account_not_found() => Ok(None),
        Err(err) => Err(err),
    }
}

pub fn init_instruction(
    task_queue_key: &Pubkey,
    task_queue: &TaskQueueV0,
    cron_id: u32,
    schedule: &str,
    name: &str,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    fn mk_accounts(
        task_queue_key: &Pubkey,
        task_id: u16,
        job_id: u32,
        name: &str,
        payer: &Pubkey,
    ) -> impl ToAccountMetas {
        let queue_authority = tuktuk::task_queue::queue_authority_key(&hpl_crons::ID);
        let task_queue_authority =
            queue::task_queue_authority_key(task_queue_key, &queue_authority);
        let user_authority = *payer;
        let authority = entity_cron_authority_key(&user_authority);
        let cron_job = tuktuk::cron::cron_job_key(&authority, job_id);
        hpl_crons::client::accounts::InitEntityClaimCronV0 {
            payer: *payer,
            queue_authority,
            task_queue_authority,
            user_authority,
            authority,
            user_cron_jobs: tuktuk::cron::user_cron_jobs_key(&authority),
            cron_job,
            cron_job_name_mapping: tuktuk::cron::name_mapping_key(&authority, name),
            task_queue: *task_queue_key,
            task: tuktuk::task::key(task_queue_key, task_id),
            task_return_account_1: tuktuk::cron::task_return_account_1_key(&cron_job),
            task_return_account_2: tuktuk::cron::task_return_account_2_key(&cron_job),
            cron_program: tuktuk_program::cron::ID,
            tuktuk_program: tuktuk_program::tuktuk::ID,
            system_program: solana_sdk::system_program::ID,
        }
    }

    let task_id = tuktuk::task_queue::next_available_task_ids_from(task_queue, 1, 0)?[0];
    let accounts = mk_accounts(task_queue_key, task_id, cron_id, name, payer);
    let ix = Instruction {
        program_id: hpl_crons::ID,
        accounts: accounts.to_account_metas(None),
        data: hpl_crons::client::args::InitEntityClaimCronV0 {
            args: hpl_crons::types::InitEntityClaimCronArgsV0 {
                schedule: schedule.to_string(),
            },
        }
        .data(),
    };
    Ok(ix)
}

pub async fn init<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    task_queue_key: &Pubkey,
    cron_id: u32,
    (schedule, name): (&str, &str),
    fund: Option<u64>,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let task_queue = client.anchor_account(task_queue_key).await?;

    let ix = init_instruction(
        task_queue_key,
        &task_queue,
        cron_id,
        schedule,
        name,
        &keypair.pubkey(),
    )?;

    let payer = keypair.pubkey();
    let cron_job_key = cron_job_key_for_wallet(&payer, cron_id);
    let fund_ix = fund.map(|amount| system_instruction::transfer(&payer, &cron_job_key, amount));

    let ixs = [
        Some(priority_fee::compute_budget_instruction(500_000)),
        Some(
            priority_fee::compute_price_instruction_for_accounts(
                client,
                &ix.accounts,
                opts.fee_range(),
            )
            .await?,
        ),
        Some(ix),
        fund_ix,
    ]
    .into_iter()
    .flatten()
    .collect_vec();

    let (msg, block_height) =
        message::mk_message(client, &ixs, &opts.lut_addresses, &keypair.pubkey()).await?;
    let txn = mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
}

pub fn requeue_instruction(
    task_queue_key: &Pubkey,
    task_queue: &TaskQueueV0,
    cron_id: u32,
    name: &str,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    fn mk_accounts(
        task_queue_key: &Pubkey,
        task_id: u16,
        job_id: u32,
        name: &str,
        payer: &Pubkey,
    ) -> impl ToAccountMetas {
        let queue_authority = tuktuk::task_queue::queue_authority_key(&hpl_crons::ID);
        let task_queue_authority =
            queue::task_queue_authority_key(task_queue_key, &queue_authority);
        let user_authority = *payer;
        let authority = entity_cron_authority_key(&user_authority);
        let cron_job = tuktuk::cron::cron_job_key(&authority, job_id);
        hpl_crons::client::accounts::RequeueEntityClaimCronV0 {
            payer: *payer,
            queue_authority,
            task_queue_authority,
            user_authority,
            authority,
            user_cron_jobs: tuktuk::cron::user_cron_jobs_key(&authority),
            cron_job,
            cron_job_name_mapping: tuktuk::cron::name_mapping_key(&authority, name),
            task_queue: *task_queue_key,
            task: tuktuk::task::key(task_queue_key, task_id),
            task_return_account_1: tuktuk::cron::task_return_account_1_key(&cron_job),
            task_return_account_2: tuktuk::cron::task_return_account_2_key(&cron_job),
            cron_program: tuktuk_program::cron::ID,
            tuktuk_program: tuktuk_program::tuktuk::ID,
            system_program: solana_sdk::system_program::ID,
        }
    }

    let task_id = tuktuk::task_queue::next_available_task_ids_from(task_queue, 1, 0)?[0];
    let accounts = mk_accounts(task_queue_key, task_id, cron_id, name, payer);
    let ix = Instruction {
        program_id: hpl_crons::ID,
        accounts: accounts.to_account_metas(None),
        data: hpl_crons::client::args::RequeueEntityClaimCronV0 {}.data(),
    };
    Ok(ix)
}

pub async fn requeue<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    task_queue_key: &Pubkey,
    cron_id: u32,
    name: &str,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let task_queue = client.anchor_account(task_queue_key).await?;

    let ix = requeue_instruction(
        task_queue_key,
        &task_queue,
        cron_id,
        name,
        &keypair.pubkey(),
    )?;

    let ixs = [
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
        message::mk_message(client, &ixs, &opts.lut_addresses, &keypair.pubkey()).await?;
    let txn = mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
}

pub const CU_CLOSE: u32 = 60_000;

pub fn close_instruction(cron_id: u32, name: &str, payer: &Pubkey) -> Result<Instruction, Error> {
    fn mk_accounts(job_id: u32, name: &str, payer: &Pubkey) -> impl ToAccountMetas {
        let user_authority = *payer;
        let authority = entity_cron_authority_key(&user_authority);
        let cron_job = tuktuk::cron::cron_job_key(&authority, job_id);
        hpl_crons::client::accounts::CloseEntityClaimCronV0 {
            payer: *payer,
            rent_refund: *payer,
            user_authority,
            authority,
            user_cron_jobs: tuktuk::cron::user_cron_jobs_key(&authority),
            cron_job,
            cron_job_name_mapping: tuktuk::cron::name_mapping_key(&authority, name),
            task_return_account_1: tuktuk::cron::task_return_account_1_key(&cron_job),
            task_return_account_2: tuktuk::cron::task_return_account_2_key(&cron_job),
            cron_program: tuktuk_program::cron::ID,
            tuktuk_program: tuktuk_program::tuktuk::ID,
            system_program: solana_sdk::system_program::ID,
        }
    }

    let accounts = mk_accounts(cron_id, name, payer);
    let ix = Instruction {
        program_id: hpl_crons::ID,
        accounts: accounts.to_account_metas(None),
        data: hpl_crons::client::args::CloseEntityClaimCronV0 {}.data(),
    };
    Ok(ix)
}

pub const CU_CLOSE_ENTITY_CLAIM: u32 = 40_000;

pub fn close_entity_claim_instruction(
    cron_job_key: &Pubkey,
    cron_job_index: u32,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    fn mk_accounts(
        cron_job_key: &Pubkey,
        cron_job_index: u32,
        payer: &Pubkey,
    ) -> impl ToAccountMetas {
        let user_authority = *payer;
        let authority = entity_cron_authority_key(&user_authority);
        hpl_crons::client::accounts::RemoveEntityFromCronV0 {
            cron_job_transaction: tuktuk::cron_job_transaction::key(cron_job_key, cron_job_index),
            rent_refund: *payer,
            user_authority,
            authority,
            cron_job: *cron_job_key,
            cron_program: tuktuk_program::cron::ID,
            system_program: solana_sdk::system_program::ID,
        }
    }

    let accounts = mk_accounts(cron_job_key, cron_job_index, payer);
    let ix = Instruction {
        program_id: hpl_crons::ID,
        accounts: accounts.to_account_metas(None),
        data: hpl_crons::client::args::RemoveEntityFromCronV0 {
            args: RemoveEntityFromCronArgsV0 {
                index: cron_job_index,
            },
        }
        .data(),
    };
    Ok(ix)
}

pub async fn close<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    cron_job_key: &Pubkey,
    cron_id: u32,
    name: &str,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let cron_job: CronJobV0 = client.anchor_account(cron_job_key).await?;

    let payer = keypair.pubkey();
    let mut ix_accounts = vec![];
    let mut compute_budget = 0;
    let close_claim_ixs: Vec<Instruction> = (0..cron_job.next_transaction_id)
        .map(|cron_job_index| {
            close_entity_claim_instruction(cron_job_key, cron_job_index, &payer).inspect(|ix| {
                ix_accounts.extend_from_slice(&ix.accounts);
                compute_budget += CU_CLOSE_ENTITY_CLAIM;
            })
        })
        .try_collect()?;

    let close_ix = close_instruction(cron_id, name, &keypair.pubkey())?;
    ix_accounts.extend_from_slice(&close_ix.accounts);
    compute_budget += CU_CLOSE;

    let ixs = &[
        &[
            priority_fee::compute_budget_instruction(compute_budget),
            priority_fee::compute_price_instruction_for_accounts(
                client,
                &ix_accounts,
                opts.fee_range(),
            )
            .await?,
        ],
        close_claim_ixs.as_slice(),
        &[close_ix],
    ]
    .concat();

    let (msg, block_height) = message::mk_message(
        client,
        ixs.as_slice(),
        &opts.lut_addresses,
        &keypair.pubkey(),
    )
    .await?;
    let txn = mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
}

pub fn claim_wallet_instruction(
    cron_job_key: &Pubkey,
    cron_job: &CronJobV0,
    wallet: &Pubkey,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    fn mk_accounts(
        cron_job_key: &Pubkey,
        cron_job_index: u32,
        wallet: &Pubkey,
        payer: &Pubkey,
    ) -> impl ToAccountMetas {
        let user_authority = *payer;
        let authority = entity_cron_authority_key(&user_authority);
        let cron_job_transaction = tuktuk::cron_job_transaction::key(cron_job_key, cron_job_index);
        hpl_crons::client::accounts::AddWalletToEntityCronV0 {
            authority,
            user_authority,
            payer: *payer,
            wallet: *wallet,
            cron_job: *cron_job_key,
            cron_job_transaction,
            system_program: solana_sdk::system_program::ID,
            cron_program: tuktuk_program::cron::ID,
        }
    }
    let cron_job_index = cron_job.next_transaction_id;
    let accounts = mk_accounts(cron_job_key, cron_job_index, wallet, payer);
    let ix = Instruction {
        program_id: hpl_crons::ID,
        accounts: accounts.to_account_metas(None),
        data: hpl_crons::client::args::AddWalletToEntityCronV0 {
            args: hpl_crons::types::AddWalletToEntityCronArgsV0 {
                index: cron_job_index,
            },
        }
        .data(),
    };
    Ok(ix)
}

pub async fn claim_wallet<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    cron_job_key: &Pubkey,
    wallet: &Pubkey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let cron_job = client.anchor_account(cron_job_key).await?;
    let ix = claim_wallet_instruction(cron_job_key, &cron_job, wallet, &keypair.pubkey())?;
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

pub fn claim_asset_instruction(
    cron_job_key: &Pubkey,
    cron_job: &CronJobV0,
    kta_key: &Pubkey,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    fn mk_accounts(
        cron_job_key: &Pubkey,
        cron_job_index: u32,
        kta_key: &Pubkey,
        payer: &Pubkey,
    ) -> impl ToAccountMetas {
        let user_authority = *payer;
        let authority = entity_cron_authority_key(&user_authority);
        let cron_job_transaction = tuktuk::cron_job_transaction::key(cron_job_key, cron_job_index);
        hpl_crons::client::accounts::AddEntityToCronV0 {
            authority,
            user_authority,
            key_to_asset: *kta_key,
            payer: *payer,
            cron_job: *cron_job_key,
            cron_job_transaction,
            system_program: solana_sdk::system_program::ID,
            cron_program: tuktuk_program::cron::ID,
        }
    }
    let cron_job_index = cron_job.next_transaction_id;
    let accounts = mk_accounts(cron_job_key, cron_job_index, kta_key, payer);
    let ix = Instruction {
        program_id: hpl_crons::ID,
        accounts: accounts.to_account_metas(None),
        data: hpl_crons::client::args::AddEntityToCronV0 {
            args: hpl_crons::types::AddEntityToCronArgsV0 {
                index: cron_job_index,
            },
        }
        .data(),
    };
    Ok(ix)
}

pub async fn claim_asset<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    cron_job_key: &Pubkey,
    encoded_entity_key: &EncodedEntityKey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let cron_job = client.anchor_account(cron_job_key).await?;
    let entity_key = encoded_entity_key.as_entity_key()?;
    let kta_key = Dao::Hnt.entity_key_to_kta_key(&entity_key);
    let ix = claim_asset_instruction(cron_job_key, &cron_job, &kta_key, &keypair.pubkey())?;
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
