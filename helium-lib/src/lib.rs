pub mod asset;
pub mod b64;
pub mod client;

pub mod boosting;
pub mod dao;
pub mod dc;
pub mod entity_key;
pub mod error;
pub mod hotspot;
pub mod keypair;
pub mod kta;
pub mod memo;
pub mod onboarding;
pub mod priority_fee;
pub mod programs;
pub mod reward;
pub mod token;

pub use anchor_client;
pub use anchor_client::solana_client;
pub use anchor_spl;
pub use client::SolanaRpcClient;
pub use helium_anchor_gen::{
    anchor_lang, circuit_breaker, data_credits, helium_entity_manager, helium_sub_daos,
    hexboosting, lazy_distributor, rewards_oracle,
};
pub use send_txn::WithTransactionSender;
pub use solana_sdk;
pub use solana_sdk::bs58;

pub(crate) trait Zero {
    const ZERO: Self;
}

impl Zero for u32 {
    const ZERO: Self = 0;
}

impl Zero for i32 {
    const ZERO: Self = 0;
}

impl Zero for u16 {
    const ZERO: Self = 0;
}

impl Zero for rust_decimal::Decimal {
    const ZERO: Self = rust_decimal::Decimal::ZERO;
}

pub(crate) fn is_zero<T>(value: &T) -> bool
where
    T: PartialEq + Zero,
{
    value == &T::ZERO
}

use anchor_client::solana_client::rpc_client::SerializableTransaction;
use error::Error;
use keypair::Pubkey;
use solana_sdk::{instruction::Instruction, signature::Signature};
use std::{sync::Arc, thread};

pub fn init(solana_client: Arc<client::SolanaRpcClient>) -> Result<(), error::Error> {
    kta::init(solana_client)
}

#[derive(Debug, Clone, Copy)]
pub struct TransactionOpts {
    pub min_priority_fee: u64,
}

impl Default for TransactionOpts {
    fn default() -> Self {
        Self {
            min_priority_fee: priority_fee::MIN_PRIORITY_FEE,
        }
    }
}

#[derive(Clone)]
pub struct TransactionWithBlockhash {
    pub inner: solana_sdk::transaction::Transaction,
    pub block_height: u64,
}

/// A sent `TransactionWithBlockhash`
#[derive(Clone)]
pub struct TrackedTransaction {
    pub txn: TransactionWithBlockhash,
    pub signature: Signature,
    pub sent_block_height: u64,
}

impl TransactionWithBlockhash {
    pub fn inner_txn(&self) -> &solana_sdk::transaction::Transaction {
        &self.inner
    }

    pub fn try_sign<T: solana_sdk::signers::Signers + ?Sized>(
        &mut self,
        keypairs: &T,
    ) -> Result<(), solana_sdk::signer::SignerError> {
        let recent_blockhash = self.inner.get_recent_blockhash();
        self.inner.try_sign(keypairs, *recent_blockhash)?;
        Ok(())
    }

    pub fn try_partial_sign<T: solana_sdk::signers::Signers + ?Sized>(
        &mut self,
        keypairs: &T,
    ) -> Result<(), solana_sdk::signer::SignerError> {
        let recent_blockhash = self.inner.get_recent_blockhash();
        self.inner.try_partial_sign(keypairs, *recent_blockhash)?;
        Ok(())
    }

    pub fn with_signed_transaction(self, txn: solana_sdk::transaction::Transaction) -> Self {
        Self {
            inner: txn,
            block_height: self.block_height,
        }
    }
}

pub async fn mk_transaction_with_blockhash<C: AsRef<SolanaRpcClient>>(
    client: &C,
    ixs: &[Instruction],
    payer: &Pubkey,
) -> Result<TransactionWithBlockhash, Error> {
    let mut txn = solana_sdk::transaction::Transaction::new_with_payer(ixs, Some(payer));
    let solana_client = AsRef::<SolanaRpcClient>::as_ref(client);
    let (latest_blockhash, latest_block_height) = solana_client
        .get_latest_blockhash_with_commitment(solana_client.commitment())
        .await?;
    txn.message.recent_blockhash = latest_blockhash;
    Ok(TransactionWithBlockhash {
        inner: txn,
        block_height: latest_block_height,
    })
}

pub mod send_txn {
    use super::*;

    use solana_client::rpc_config::RpcSendTransactionConfig;
    use solana_sdk::commitment_config::CommitmentConfig;
    use std::time::Duration;

    type LibError = solana_client::client_error::ClientError;

    pub struct Sender<'a, C> {
        client: &'a C,
        txn: &'a TransactionWithBlockhash,
        finalize: bool,
        retry: Option<(usize, Duration)>,
    }

    #[async_trait::async_trait]
    pub trait SenderExt: Sized + Send + Sync {
        async fn send_txn(
            &self,
            txn: &TransactionWithBlockhash,
            config: RpcSendTransactionConfig,
        ) -> Result<Signature, LibError>;
        async fn finalize_signature(&self, signature: &Signature) -> Result<(), LibError>;
        async fn get_block_height(&self) -> Result<u64, LibError>;

        // Override if you need a tokio sleep
        async fn sleep(&self, delay: Duration) {
            thread::sleep(delay);
        }
    }

    pub trait WithTransactionSender: Sized + Send + Sync {
        fn with_transaction<'a>(&'a self, txn: &'a TransactionWithBlockhash) -> Sender<'a, Self>;

        fn with_finalized_transaction<'a>(
            &'a self,
            txn: &'a TransactionWithBlockhash,
        ) -> Sender<'a, Self>;
    }

    impl<C: SenderExt> Sender<'_, C> {
        pub fn finalized(&mut self) -> &mut Self {
            self.finalize = true;
            self
        }

        pub fn with_finalize(&mut self, finalize: bool) -> &mut Self {
            self.finalize = finalize;
            self
        }

        pub fn with_retry(&mut self, max_attempts: usize, retry_delay: Duration) -> &mut Self {
            self.retry = Some((max_attempts, retry_delay));
            self
        }

        pub async fn send(
            &self,
            config: RpcSendTransactionConfig,
        ) -> Result<TrackedTransaction, LibError> {
            let sent_block_height = self.client.get_block_height().await?;
            let signature = self.send_with_retry(config).await?;

            if self.finalize {
                self.client.finalize_signature(&signature).await?;
            }

            Ok(TrackedTransaction {
                txn: self.txn.clone(),
                signature,
                sent_block_height,
            })
        }

        async fn send_with_retry(
            &self,
            config: RpcSendTransactionConfig,
        ) -> Result<Signature, LibError> {
            let (max_retry, retry_delay) = self.retry.unwrap_or((1, Duration::from_millis(0)));
            let mut attempt = 0;

            loop {
                match self.client.send_txn(self.txn, config).await {
                    Ok(sig) => return Ok(sig),
                    Err(err) => {
                        attempt += 1;
                        if attempt == max_retry {
                            return Err(err);
                        }
                        self.client.sleep(retry_delay).await;
                    }
                }
            }
        }
    }

    // Default impl for anything that can send transactions
    impl<T: SenderExt> WithTransactionSender for T {
        fn with_transaction<'a>(&'a self, txn: &'a TransactionWithBlockhash) -> Sender<'a, T> {
            Sender {
                client: self,
                txn,
                finalize: false,
                retry: None,
            }
        }
        fn with_finalized_transaction<'a>(
            &'a self,
            txn: &'a TransactionWithBlockhash,
        ) -> Sender<'a, Self> {
            Sender {
                client: self,
                txn,
                finalize: true,
                retry: None,
            }
        }
    }

    // Default impl for anything that can be turned into a `SolanaRpcClient`
    #[async_trait::async_trait]
    impl<T: AsRef<SolanaRpcClient> + Send + Sync> SenderExt for T {
        async fn send_txn(
            &self,
            txn: &TransactionWithBlockhash,
            config: RpcSendTransactionConfig,
        ) -> Result<Signature, LibError> {
            Ok(self
                .as_ref()
                .send_transaction_with_config(txn.inner_txn(), config)
                .await?)
        }

        async fn finalize_signature(&self, signature: &Signature) -> Result<(), LibError> {
            Ok(self
                .as_ref()
                .poll_for_signature_with_commitment(signature, CommitmentConfig::finalized())
                .await?)
        }

        async fn get_block_height(&self) -> Result<u64, LibError> {
            Ok(self.as_ref().get_block_height().await?)
        }
    }
}
