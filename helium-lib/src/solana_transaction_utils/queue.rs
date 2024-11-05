use solana_sdk::{
    commitment_config::CommitmentConfig, instruction::Instruction, message::Message,
    signature::Keypair, signer::Signer, transaction::TransactionError,
};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::mpsc::{channel, Receiver, Sender},
    task::JoinHandle,
    time::interval,
};

use crate::{
    solana_client::{
        nonblocking::{rpc_client::RpcClient, tpu_client::TpuClient},
        send_and_confirm_transactions_in_parallel::{
            send_and_confirm_transactions_in_parallel, SendAndConfirmConfig,
        },
        tpu_client::TpuClientConfig,
    },
    solana_transaction_utils::{
        pack::pack_instructions_into_transactions, priority_fee::auto_compute_limit_and_price,
    },
};

#[derive(Debug, Clone)]
pub struct TransactionTask<T: Send + Clone> {
    pub task: T,
    pub instructions: Vec<Instruction>,
}

#[derive(Debug)]
pub struct CompletedTransactionTask<T: Send + Clone> {
    pub err: Option<TransactionError>,
    pub task: TransactionTask<T>,
}

pub struct TransactionQueueArgs<T: Send + Clone> {
    pub rpc_client: Arc<RpcClient>,
    pub ws_url: String,
    pub payer: Arc<Keypair>,
    pub batch_duration: Duration,
    pub receiver: Receiver<TransactionTask<T>>,
    pub result_sender: Sender<CompletedTransactionTask<T>>,
}

pub struct TransactionQueueHandles<T: Send + Clone> {
    pub sender: Sender<TransactionTask<T>>,
    pub receiver: Receiver<TransactionTask<T>>,
    pub result_sender: Sender<CompletedTransactionTask<T>>,
    pub result_receiver: Receiver<CompletedTransactionTask<T>>,
}

pub fn create_transaction_queue_handles<T: Send + Clone>(
    channel_capacity: usize,
) -> TransactionQueueHandles<T> {
    let (tx, rx) = channel::<TransactionTask<T>>(channel_capacity);
    let (result_tx, result_rx) = channel::<CompletedTransactionTask<T>>(channel_capacity);
    TransactionQueueHandles {
        sender: tx,
        receiver: rx,
        result_sender: result_tx,
        result_receiver: result_rx,
    }
}

pub fn create_transaction_queue<T: Send + Clone + 'static>(
    args: TransactionQueueArgs<T>,
) -> JoinHandle<()> {
    let TransactionQueueArgs {
        rpc_client,
        payer,
        batch_duration,
        ws_url,
        receiver: mut rx,
        result_sender: result_tx,
    } = args;
    let thread: JoinHandle<()> = tokio::spawn(async move {
        let mut tasks: Vec<TransactionTask<T>> = Vec::new();
        let mut interval = interval(batch_duration);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if tasks.is_empty() {
                        continue;
                    }
                    let rpc_client = rpc_client.clone();
                    let blockhash = rpc_client.get_latest_blockhash_with_commitment(CommitmentConfig::finalized()).await.expect("Failed to get latest blockhash");
                    let tpu_client = TpuClient::new("helium-transaction-queue", rpc_client.clone(), ws_url.as_str(), TpuClientConfig::default()).await.expect("Failed to create TPU client");
                    // Process the collected tasks here
                    let ix_groups: Vec<Vec<Instruction>> = tasks.clone().into_iter().map(|t| t.instructions).collect();
                    let txs = pack_instructions_into_transactions(ix_groups, &payer);
                    let mut with_auto_compute: Vec<Message> = Vec::new();
                    for (tx, _) in &txs {
                        // This is just a tx with compute ixs. Skip it
                        if tx.len() == 2 {
                            continue;
                        }
                        let computed = auto_compute_limit_and_price(&rpc_client, tx.clone(), &[&payer], 1.2, Some(&payer.pubkey()), Some(blockhash.0)).await.unwrap();
                        with_auto_compute.push(Message::new(&computed, Some(&payer.pubkey())));
                    }
                    if with_auto_compute.is_empty() {
                        continue;
                    }
                    let results = send_and_confirm_transactions_in_parallel(
                        rpc_client.clone(),
                        Some(tpu_client),
                        &with_auto_compute,
                        &[&payer],
                        SendAndConfirmConfig {
                            with_spinner: true,
                        resign_txs_count: Some(5),
                        },
                    ).await.expect("Failed to send txs");
                    let mut task_results: std::collections::HashMap<usize, Option<TransactionError>> = std::collections::HashMap::new();
                    for (i, result) in results.iter().enumerate() {
                        for task_id in &txs[i].1 {
                                if let Some(err) = result {
                                    task_results.insert(*task_id, Some(err.clone()));
                                } else if !task_results.contains_key(task_id) {
                                    task_results.insert(*task_id, None);
                                }
                        }
                    }
                    for (task_id, err) in task_results {
                        result_tx.send(CompletedTransactionTask {
                            err,
                            task: tasks[task_id].clone(),
                        }).await.unwrap();
                    }
                    tasks.clear();
                }

                Some(task) = rx.recv() => {
                    tasks.push(task);
                }
            }
        }
    });
    thread
}
