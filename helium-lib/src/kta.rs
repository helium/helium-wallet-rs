use crate::{
    dao::Dao,
    entity_key::AsEntityKey,
    keypair::{Keypair, Pubkey, VoidKeypair},
    result::{Error, Result},
    settings::Settings,
};
use anchor_client::anchor_lang::AccountDeserialize;
use helium_anchor_gen::helium_entity_manager::{self, KeyToAssetV0};
use itertools::Itertools;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

pub fn init(settings: &Settings) -> Result<()> {
    let _ = CACHE.set(KtaCache::new(settings)?);
    Ok(())
}

pub async fn get(kta_key: &Pubkey) -> Result<KeyToAssetV0> {
    let cache = CACHE
        .get()
        .ok_or_else(|| anchor_client::ClientError::AccountNotFound)?;
    cache.get(kta_key).await
}

pub async fn get_many(kta_keys: &[Pubkey]) -> Result<Vec<KeyToAssetV0>> {
    let cache = CACHE
        .get()
        .ok_or_else(|| anchor_client::ClientError::AccountNotFound)?;
    cache.get_many(kta_keys).await
}

pub async fn for_entity_key<E>(entity_key: &E) -> Result<KeyToAssetV0>
where
    E: AsEntityKey,
{
    let kta_key = Dao::Hnt.entity_key_to_kta_key(entity_key);
    get(&kta_key).await
}

static CACHE: OnceLock<KtaCache> = OnceLock::new();

type KtaCacheMap = HashMap<Pubkey, KeyToAssetV0>;
struct KtaCache {
    program: anchor_client::Program<Arc<VoidKeypair>>,
    cache: RwLock<KtaCacheMap>,
}

impl KtaCache {
    fn new(settings: &Settings) -> Result<Self> {
        let anchor_client = settings.mk_anchor_client(Keypair::void())?;
        let program = anchor_client.program(helium_entity_manager::id())?;
        let cache = RwLock::new(KtaCacheMap::new());
        Ok(Self { program, cache })
    }

    fn cache_read(&self) -> RwLockReadGuard<'_, KtaCacheMap> {
        self.cache.read().expect("cache read lock poisoned")
    }

    fn cache_write(&self) -> RwLockWriteGuard<'_, KtaCacheMap> {
        self.cache.write().expect("cache write lock poisoned")
    }

    async fn get(&self, kta_key: &Pubkey) -> Result<helium_entity_manager::KeyToAssetV0> {
        if let Some(account) = self.cache_read().get(kta_key) {
            return Ok(account.clone());
        }

        let kta = self
            .program
            .account::<helium_entity_manager::KeyToAssetV0>(*kta_key)
            .await?;
        self.cache_write().insert(*kta_key, kta.clone());
        Ok(kta)
    }

    async fn get_many(
        &self,
        kta_keys: &[Pubkey],
    ) -> Result<Vec<helium_entity_manager::KeyToAssetV0>> {
        let missing_keys: Vec<Pubkey> = {
            let cache = self.cache_read();
            kta_keys
                .iter()
                .filter(|key| !cache.contains_key(key))
                .copied()
                .collect()
        };
        let mut missing_accounts = self
            .program
            .async_rpc()
            .get_multiple_accounts(&missing_keys)
            .await?;
        {
            let mut cache = self.cache_write();
            missing_keys
                .into_iter()
                .zip(missing_accounts.iter_mut())
                .map(|(key, maybe_account)| {
                    let Some(account) = maybe_account.as_mut() else {
                        return Err(Error::from(anchor_client::ClientError::AccountNotFound));
                    };
                    helium_entity_manager::KeyToAssetV0::try_deserialize(&mut account.data.as_ref())
                        .map_err(Error::from)
                        .map(|kta| (key, kta))
                })
                .map_ok(|(key, kta)| {
                    cache.insert(key, kta);
                })
                .try_collect()?;
        }
        {
            let cache = self.cache_read();
            kta_keys
                .iter()
                .map(|key| {
                    cache
                        .get(key)
                        .cloned()
                        .ok_or(anchor_client::ClientError::AccountNotFound.into())
                })
                .try_collect()
        }
    }
}
