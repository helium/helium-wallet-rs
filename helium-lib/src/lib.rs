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
pub use solana_sdk::bs58;
pub use solana_sdk::{self, signature::Signature};

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
use solana_sdk::instruction::Instruction;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionWithBlockhash {
    pub inner: solana_sdk::transaction::Transaction,
    pub block_height: u64,
}

/// A sent `TransactionWithBlockhash`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedTransaction {
    pub txn: TransactionWithBlockhash,
    pub signature: Signature,
    pub sent_block_height: u64,
}

impl TransactionWithBlockhash {
    pub fn inner_txn(&self) -> &solana_sdk::transaction::Transaction {
        &self.inner
    }

    pub fn get_signature(&self) -> &Signature {
        self.inner.get_signature()
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
    use tracing::Instrument;

    /// Top Level Error for TxnSender.
    #[derive(Debug, thiserror::Error)]
    pub enum TxnSenderError {
        #[error("store callback failed: {0}")]
        Store(#[from] TxnStorePrepareError),
        #[error("solana client failed: {0}")]
        Client(#[from] solana_client::client_error::ClientError),
        #[error("could not submit {attempts} times")]
        Submission { attempts: usize },
        #[error("could not finalize")]
        Finalization,
    }

    pub type SolanaClientError = solana_client::client_error::ClientError;

    #[derive(Debug, thiserror::Error)]
    #[error("TxnStore failed on_prepare: {0}")]
    pub struct TxnStorePrepareError(String);

    impl TxnStorePrepareError {
        pub fn new<T: std::fmt::Display>(err_msg: T) -> Self {
            Self(err_msg.to_string())
        }
    }

    /// A trait for dealing with in flight transactions.
    ///
    /// Default implementation `NoopStore` provided with `TxnSender::new()`
    /// Returning an error from `on_prepare` will stop a Txn from being submitted.
    #[async_trait::async_trait]
    pub trait TxnStore: Send + Sync {
        fn make_span(&self) -> tracing::Span;
        async fn on_prepared(&self, signature: &Signature) -> Result<(), TxnStorePrepareError>;
        async fn on_sent(&self, signature: &Signature);
        async fn on_sent_retry(&self, signature: &Signature, attempt: usize);
        async fn on_finalized(&self, signature: &Signature);
        async fn on_error(&self, signature: &Signature, err: TxnSenderError);
    }

    /// A trait for sleeping in between `send_txn` attempts.
    ///
    /// Default implementation `BlockingSleeper` provided with `TxnSender::new()`.
    ///
    /// If you have tokio available, a non-blocking sleeper can be implemented.
    /// ```no_compile
    /// struct AsyncSleeper;
    ///
    /// #[async_trait::async_trait]
    /// impl helium_lib::send_txn::TxnSleeper for AsyncSleeper {
    ///     async fn sleep(&self, duration: Duration) {
    ///         tokio::time::sleep(duration).await;
    ///     }
    /// }
    /// ```
    #[async_trait::async_trait]
    pub trait TxnSleeper {
        async fn sleep(&self, duration: Duration);
    }

    /// TxnSender wraps all the needed information to send and finalize a Solana Transaction.
    pub struct TxnSender<'a, Client, Store = NoopStore, Sleeper = BlockingSleeper> {
        client: &'a Client,
        txn: &'a TransactionWithBlockhash,
        finalize: bool,
        retry: Option<(usize, Duration)>,
        store: &'a Store,
        sleeper: Sleeper,
    }

    /// A trait for Solana Clients to implement for sending transactions.
    #[async_trait::async_trait]
    pub trait TxnSenderClientExt: Sized + Send + Sync {
        async fn send_txn(
            &self,
            txn: &TransactionWithBlockhash,
            config: RpcSendTransactionConfig,
        ) -> Result<Signature, SolanaClientError>;
        async fn finalize_signature(&self, signature: &Signature) -> Result<(), SolanaClientError>;
        async fn get_block_height(&self) -> Result<u64, SolanaClientError>;
    }

    /// Constructor methods for `TxnSender` providing the default Store and Sleeper implementations.
    impl<'a, C: TxnSenderClientExt> TxnSender<'a, C> {
        pub fn new(client: &'a C, txn: &'a TransactionWithBlockhash) -> TxnSender<'a, C> {
            TxnSender::<'a, C> {
                client,
                txn,
                finalize: false,
                retry: None,
                store: &NoopStore,
                sleeper: BlockingSleeper,
            }
        }
    }

    /// Builder methods API for updating TxnSender with options.
    impl<'a, Client: TxnSenderClientExt, Store: TxnStore, Sleeper: TxnSleeper>
        TxnSender<'a, Client, Store, Sleeper>
    {
        pub fn finalized(mut self, finalize: bool) -> Self {
            self.finalize = finalize;
            self
        }

        pub fn with_retry(&mut self, max_attempts: usize, retry_delay: Duration) -> &mut Self {
            self.retry = Some((max_attempts, retry_delay));
            self
        }

        pub fn with_store<S2: TxnStore>(self, store: &'a S2) -> TxnSender<'a, Client, S2, Sleeper> {
            TxnSender {
                client: self.client,
                txn: self.txn,
                finalize: self.finalize,
                retry: self.retry,
                sleeper: self.sleeper,
                store,
            }
        }

        pub fn with_sleeper<S2: TxnSleeper>(self, sleeper: S2) -> TxnSender<'a, Client, Store, S2> {
            TxnSender {
                client: self.client,
                txn: self.txn,
                finalize: self.finalize,
                retry: self.retry,
                store: self.store,
                sleeper,
            }
        }
    }

    /// Functionality for TxnSender to sending and finalizing txns.
    impl<'a, Client: TxnSenderClientExt, Store: TxnStore, Sleeper: TxnSleeper>
        TxnSender<'a, Client, Store, Sleeper>
    {
        pub async fn send(
            &self,
            config: RpcSendTransactionConfig,
        ) -> Result<TrackedTransaction, TxnSenderError> {
            let span = self.store.make_span();
            self.do_send(config).instrument(span).await
        }

        pub async fn do_send(
            &self,
            config: RpcSendTransactionConfig,
        ) -> Result<TrackedTransaction, TxnSenderError> {
            let sent_block_height = self.client.get_block_height().await?;
            self.store.on_prepared(self.txn.get_signature()).await?;

            let signature = self.send_with_retry(config).await?;
            self.store.on_sent(&signature).await;

            if self.finalize {
                self.finalize(&signature).await?;
                self.store.on_finalized(&signature).await;
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
        ) -> Result<Signature, TxnSenderError> {
            let (max_retry, retry_delay) = self.retry.unwrap_or((1, Duration::from_millis(0)));
            let mut attempt = 0;

            loop {
                match self.client.send_txn(self.txn, config).await {
                    Ok(sig) => return Ok(sig),
                    Err(err) => {
                        let sig = self.txn.inner_txn().get_signature();
                        attempt += 1;
                        if attempt == max_retry {
                            self.store
                                .on_error(sig, TxnSenderError::Submission { attempts: attempt })
                                .await;
                            return Err(err.into());
                        }
                        self.sleeper.sleep(retry_delay).await;
                        self.store.on_sent_retry(&sig, attempt).await;
                    }
                }
            }
        }

        async fn finalize(&self, signature: &Signature) -> Result<(), TxnSenderError> {
            if let Err(err) = self.client.finalize_signature(signature).await {
                self.store
                    .on_error(signature, TxnSenderError::Finalization)
                    .await;

                return Err(err.into());
            }
            Ok(())
        }
    }

    /// Default impl for anything that can be turned into a `SolanaRpcClient`
    #[async_trait::async_trait]
    impl<T: AsRef<SolanaRpcClient> + Send + Sync> TxnSenderClientExt for T {
        async fn send_txn(
            &self,
            txn: &TransactionWithBlockhash,
            config: RpcSendTransactionConfig,
        ) -> Result<Signature, SolanaClientError> {
            Ok(self
                .as_ref()
                .send_transaction_with_config(txn.inner_txn(), config)
                .await?)
        }

        async fn finalize_signature(&self, signature: &Signature) -> Result<(), SolanaClientError> {
            // TODO: poll while checking against block height.
            // Maybe return a max_block_height_surpassed error.
            Ok(self
                .as_ref()
                .poll_for_signature_with_commitment(signature, CommitmentConfig::finalized())
                .await?)
        }

        async fn get_block_height(&self) -> Result<u64, SolanaClientError> {
            Ok(self.as_ref().get_block_height().await?)
        }
    }

    /// Default store for `TxnSender`
    pub struct NoopStore;

    #[async_trait::async_trait]
    impl TxnStore for NoopStore {
        fn make_span(&self) -> tracing::Span {
            tracing::info_span!("NoopStore")
        }
        async fn on_prepared(&self, _signature: &Signature) -> Result<(), TxnStorePrepareError> {
            Ok(())
        }
        async fn on_sent(&self, _signature: &Signature) {}
        async fn on_sent_retry(&self, _signature: &Signature, _attempt: usize) {}
        async fn on_finalized(&self, _signature: &Signature) {}
        async fn on_error(&self, _signature: &Signature, _err: TxnSenderError) {}
    }

    /// Default Sleeper for `TxnSender`.
    pub struct BlockingSleeper;

    #[async_trait::async_trait]
    impl TxnSleeper for BlockingSleeper {
        async fn sleep(&self, duration: Duration) {
            thread::sleep(duration);
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::Mutex;

        use futures::executor::block_on;
        use solana_sdk::signer::SignerError;

        use super::*;

        #[derive(Default)]
        struct MockTxnStore {
            pub fail_prepared: bool,
            pub calls: Arc<Mutex<Vec<String>>>,
        }

        impl MockTxnStore {
            fn fail_prepared() -> Self {
                Self {
                    fail_prepared: true,
                    ..Default::default()
                }
            }
            fn record_call(&self, method: String) {
                self.calls.lock().unwrap().push(method);
            }
        }

        #[async_trait::async_trait]
        impl TxnStore for MockTxnStore {
            fn make_span(&self) -> tracing::Span {
                tracing::info_span!("mock store")
            }
            async fn on_prepared(&self, signature: &Signature) -> Result<(), TxnStorePrepareError> {
                if self.fail_prepared {
                    return Err(TxnStorePrepareError::new("mock failure"));
                }
                self.record_call(format!("on_prepared: {signature}"));
                Ok(())
            }
            async fn on_sent(&self, signature: &Signature) {
                self.record_call(format!("on_sent: {signature}"));
            }
            async fn on_sent_retry(&self, signature: &Signature, attempt: usize) {
                self.record_call(format!("on_sent_retry: {attempt} {signature}"));
            }
            async fn on_finalized(&self, signature: &Signature) {
                self.record_call(format!("on_finalized: {signature}"))
            }
            async fn on_error(&self, signature: &Signature, err: TxnSenderError) {
                self.record_call(format!("on_error: {signature} {err}"))
            }
        }

        #[derive(Default)]
        struct MockClient {
            pub sent_attempts: Mutex<usize>,
            pub succeed_after_sent_attempts: usize,
            pub finalize_success: bool,
            pub block_height: u64,
        }

        impl MockClient {
            fn succeed() -> Self {
                Self {
                    sent_attempts: Mutex::new(0),
                    succeed_after_sent_attempts: 0,
                    finalize_success: true,
                    block_height: 1,
                }
            }

            fn succeed_after(succeed_after_sent_attempts: usize) -> Self {
                Self {
                    sent_attempts: Mutex::new(0),
                    succeed_after_sent_attempts,
                    finalize_success: true,
                    block_height: 1,
                }
            }
        }

        #[async_trait::async_trait]
        impl TxnSenderClientExt for MockClient {
            async fn send_txn(
                &self,
                txn: &TransactionWithBlockhash,
                _config: RpcSendTransactionConfig,
            ) -> Result<Signature, SolanaClientError> {
                let mut attempts = self.sent_attempts.lock().unwrap();
                *attempts += 1;

                if *attempts >= self.succeed_after_sent_attempts {
                    return Ok(txn.inner_txn().get_signature().clone());
                }

                // Fake Error
                Err(SignerError::KeypairPubkeyMismatch.into())
            }

            async fn finalize_signature(
                &self,
                _signature: &Signature,
            ) -> Result<(), SolanaClientError> {
                if self.finalize_success {
                    return Ok(());
                }
                // Fake Error
                Err(SignerError::KeypairPubkeyMismatch.into())
            }

            async fn get_block_height(&self) -> Result<u64, SolanaClientError> {
                Ok(self.block_height)
            }
        }

        fn mk_test_transaction() -> TransactionWithBlockhash {
            let mut inner = solana_sdk::transaction::Transaction::default();
            inner.signatures.push(Signature::new_unique());
            TransactionWithBlockhash {
                inner,
                block_height: 1,
            }
        }

        #[test]
        fn send_finalized_success() {
            let tx = mk_test_transaction();
            let store = MockTxnStore::default();
            let client = MockClient::succeed();

            let tracked = block_on(
                TxnSender::new(&client, &tx)
                    .finalized(true)
                    .with_store(&store)
                    .send(RpcSendTransactionConfig::default()),
            )
            .unwrap();

            assert_eq!(tracked.sent_block_height, 1);
            assert_eq!(tracked.signature, *tx.get_signature());
            assert_eq!(tracked.txn, tx);

            let signature = tx.get_signature();
            let calls = store.calls.lock().unwrap();
            assert_eq!(
                *calls,
                vec![
                    format!("on_prepared: {signature}"),
                    format!("on_sent: {signature}"),
                    format!("on_finalized: {signature}")
                ]
            )
        }

        #[test]
        fn send_finalized_success_after_retry() {
            let tx = mk_test_transaction();
            let store = MockTxnStore::default();
            let client = MockClient::succeed_after(5);

            let tracked = block_on(
                TxnSender::new(&client, &tx)
                    .finalized(true)
                    .with_store(&store)
                    .with_retry(5, Duration::from_millis(5))
                    .send(RpcSendTransactionConfig::default()),
            )
            .unwrap();

            assert_eq!(tracked.sent_block_height, 1);
            assert_eq!(tracked.signature, *tx.get_signature());
            assert_eq!(tracked.txn, tx);

            let signature = tx.get_signature();
            let calls = store.calls.lock().unwrap();
            assert_eq!(
                *calls,
                vec![
                    format!("on_prepared: {signature}"),
                    format!("on_sent_retry: 1 {signature}"),
                    format!("on_sent_retry: 2 {signature}"),
                    format!("on_sent_retry: 3 {signature}"),
                    format!("on_sent_retry: 4 {signature}"),
                    format!("on_sent: {signature}"),
                    format!("on_finalized: {signature}")
                ]
            );
        }

        #[test]
        fn send_error_with_retry() {
            let tx = mk_test_transaction();
            let store = MockTxnStore::default();
            let client = MockClient::succeed_after(999);

            let tracked = block_on(
                TxnSender::new(&client, &tx)
                    .with_store(&store)
                    .with_retry(5, Duration::from_millis(5))
                    .send(RpcSendTransactionConfig::default()),
            );

            assert!(tracked.is_err());

            let signature = tx.get_signature();
            let calls = store.calls.lock().unwrap();
            assert_eq!(
                *calls,
                vec![
                    format!("on_prepared: {signature}"),
                    format!("on_sent_retry: 1 {signature}"),
                    format!("on_sent_retry: 2 {signature}"),
                    format!("on_sent_retry: 3 {signature}"),
                    format!("on_sent_retry: 4 {signature}"),
                    format!("on_error: {signature} could not submit 5 times")
                ]
            );
        }

        #[test]
        fn send_success_finalize_error() {
            let tx = mk_test_transaction();
            let store = MockTxnStore::default();
            let mut client = MockClient::succeed();
            client.finalize_success = false;

            let tracked = block_on(
                TxnSender::new(&client, &tx)
                    .finalized(true)
                    .with_store(&store)
                    .send(RpcSendTransactionConfig::default()),
            );

            assert!(tracked.is_err());

            let signature = tx.get_signature();
            let calls = store.calls.lock().unwrap();
            assert_eq!(
                *calls,
                vec![
                    format!("on_prepared: {signature}"),
                    format!("on_sent: {signature}"),
                    format!("on_error: {signature} could not finalize")
                ]
            );
        }

        #[test]
        fn failed_preparation() {
            let tx = mk_test_transaction();
            let store = MockTxnStore::fail_prepared();
            let client = MockClient::succeed();

            let tracked = block_on(
                TxnSender::new(&client, &tx)
                    .finalized(true)
                    .with_store(&store)
                    .send(RpcSendTransactionConfig::default()),
            );

            assert!(tracked.is_err());

            let calls = store.calls.lock().unwrap();
            assert_eq!(*calls, Vec::<String>::new())
        }
    }
}
