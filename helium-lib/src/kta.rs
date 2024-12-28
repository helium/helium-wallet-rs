use crate::{
    anchor_lang::AccountDeserialize, client::SolanaRpcClient, dao::Dao, entity_key::AsEntityKey,
    error::Error, helium_entity_manager::KeyToAssetV0, keypair::Pubkey,
    solana_sdk::account::Account,
};
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use itertools::Itertools;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

pub fn init(solana_client: Arc<SolanaRpcClient>) -> Result<(), Error> {
    let _ = CACHE.set(KtaCache::new(solana_client)?);
    Ok(())
}

pub async fn get(kta_key: &Pubkey) -> Result<KeyToAssetV0, Error> {
    let cache = CACHE.get().ok_or_else(Error::account_not_found)?;
    cache.get(kta_key).await
}

pub async fn get_many(kta_keys: &[Pubkey]) -> Result<Vec<KeyToAssetV0>, Error> {
    let cache = CACHE.get().ok_or_else(Error::account_not_found)?;
    cache.get_many(kta_keys).await
}

pub async fn for_entity_key<E>(entity_key: &E) -> Result<KeyToAssetV0, Error>
where
    E: AsEntityKey,
{
    let kta_key = Dao::Hnt.entity_key_to_kta_key(entity_key);
    get(&kta_key).await
}

pub async fn for_entity_keys<E>(entity_keys: &[E]) -> Result<Vec<KeyToAssetV0>, Error>
where
    E: AsEntityKey,
{
    let kta_keys = entity_keys
        .iter()
        .map(|entity_key| Dao::Hnt.entity_key_to_kta_key(entity_key))
        .collect::<Vec<Pubkey>>();
    get_many(&kta_keys).await
}

static CACHE: OnceLock<KtaCache> = OnceLock::new();

type KtaCacheMap = HashMap<Pubkey, KeyToAssetV0>;
struct KtaCache {
    solana_client: Arc<SolanaRpcClient>,
    cache: RwLock<KtaCacheMap>,
}

impl KtaCache {
    fn new(solana_client: Arc<SolanaRpcClient>) -> Result<Self, Error> {
        let cache = RwLock::new(KtaCacheMap::new());
        Ok(Self {
            solana_client,
            cache,
        })
    }

    fn cache_read(&self) -> RwLockReadGuard<'_, KtaCacheMap> {
        self.cache.read().expect("cache read lock poisoned")
    }

    fn cache_write(&self) -> RwLockWriteGuard<'_, KtaCacheMap> {
        self.cache.write().expect("cache write lock poisoned")
    }

    async fn get(&self, kta_key: &Pubkey) -> Result<KeyToAssetV0, Error> {
        if let Some(account) = self.cache_read().get(kta_key) {
            return Ok(account.clone());
        }

        let kta = self
            .solana_client
            .get_account(kta_key)
            .map_err(Error::from)
            .and_then(|acc| async move {
                KeyToAssetV0::try_deserialize(&mut acc.data.as_ref()).map_err(Error::from)
            })
            .await?;
        // NOTE: Holding lock across an await will not work with std::sync
        // Since sync::RwLock is much faster than sync options we take the hit
        // of multiple requests for the same kta_key before the key is found
        self.cache_write().insert(*kta_key, kta.clone());
        Ok(kta)
    }

    async fn get_many(&self, kta_keys: &[Pubkey]) -> Result<Vec<KeyToAssetV0>, Error> {
        let missing_keys: Vec<Pubkey> = {
            let cache = self.cache_read();
            kta_keys
                .iter()
                .filter(|key| !cache.contains_key(key))
                .copied()
                .collect()
        };

        let mut missing_accounts = stream::iter(missing_keys.clone())
            // Chunk into documented max keys to pass to getMultipleAccounts
            .chunks(100)
            .map(|key_chunk| async move {
                self.solana_client
                    .get_multiple_accounts(key_chunk.as_slice())
                    .await
            })
            .buffered(5)
            .try_collect::<Vec<Vec<Option<Account>>>>()
            .await?
            .into_iter()
            .flatten()
            .collect_vec();
        {
            let mut cache = self.cache_write();
            missing_keys
                .into_iter()
                .zip(missing_accounts.iter_mut())
                .map(|(key, maybe_account)| {
                    let Some(account) = maybe_account.as_mut() else {
                        return Err(Error::account_not_found());
                    };
                    KeyToAssetV0::try_deserialize(&mut account.data.as_ref())
                        .map_err(Error::from)
                        .map(|kta| (key, kta))
                })
                .map_ok(|(key, kta)| {
                    cache.insert(key, kta);
                })
                .try_collect::<_, (), _>()?;
        }
        {
            let cache = self.cache_read();
            kta_keys
                .iter()
                .map(|key| cache.get(key).cloned().ok_or(Error::account_not_found()))
                .try_collect()
        }
    }
}
