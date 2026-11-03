use crate::{
    client::GetAnchorAccount, dao::SubDao, entity_key::AsEntityKey, error::Error,
    helium_entity_manager, hotspot::HotspotInfo, keypair::Pubkey,
};
use futures::{
    stream::{self, StreamExt, TryStreamExt},
    TryFutureExt,
};
use std::collections::HashMap;

/// Fetches on-chain hotspot info for a single sub-DAO by its info account key.
pub async fn get<C: GetAnchorAccount>(
    client: &C,
    subdao: SubDao,
    info_key: &Pubkey,
) -> Result<Option<HotspotInfo>, Error> {
    let hotspot_info = match subdao {
        SubDao::Iot => client
            .anchor_account::<helium_entity_manager::accounts::IotHotspotInfoV0>(info_key)
            .await
            .map(Into::into),
        SubDao::Mobile => client
            .anchor_account::<helium_entity_manager::accounts::MobileHotspotInfoV0>(info_key)
            .await
            .map(Into::into),
    }
    .ok();
    Ok(hotspot_info)
}

/// Fetches on-chain hotspot info for multiple info account keys in a single sub-DAO.
pub async fn get_many<C: GetAnchorAccount>(
    client: &C,
    subdao: SubDao,
    info_keys: &[Pubkey],
) -> Result<Vec<Option<HotspotInfo>>, Error> {
    fn to_infos<T: Into<HotspotInfo>>(maybe_accounts: Vec<Option<T>>) -> Vec<Option<HotspotInfo>> {
        maybe_accounts
            .into_iter()
            .map(HotspotInfo::from_maybe)
            .collect()
    }
    let accounts = match subdao {
        SubDao::Iot => to_infos(
            client
                .anchor_accounts::<helium_entity_manager::accounts::IotHotspotInfoV0>(info_keys)
                .await?,
        ),
        SubDao::Mobile => to_infos(
            client
                .anchor_accounts::<helium_entity_manager::accounts::MobileHotspotInfoV0>(info_keys)
                .await?,
        ),
    };
    Ok(accounts)
}

async fn for_entity_key_in_subdao<C: GetAnchorAccount, E: AsEntityKey>(
    client: &C,
    subdao: SubDao,
    entity_key: &E,
) -> Result<Option<HotspotInfo>, Error> {
    let info_key = subdao.info_key(entity_key);
    get(client, subdao, &info_key).await
}

/// Fetches hotspot info across multiple sub-DAOs for a given entity key.
///
/// Returns a map of sub-DAO to info for each sub-DAO the hotspot is registered in.
pub async fn for_entity_key<C: GetAnchorAccount>(
    client: &C,
    subdaos: &[SubDao],
    key: &helium_crypto::PublicKey,
) -> Result<HashMap<SubDao, HotspotInfo>, Error> {
    stream::iter(subdaos.to_vec())
        .map(|subdao| {
            for_entity_key_in_subdao(client, subdao, key)
                .map_ok(move |maybe_metadata| maybe_metadata.map(|metadata| (subdao, metadata)))
        })
        .buffer_unordered(10)
        .filter_map(|result| async move { result.transpose() })
        .try_collect::<Vec<(SubDao, HotspotInfo)>>()
        .map_ok(HashMap::from_iter)
        .await
}
