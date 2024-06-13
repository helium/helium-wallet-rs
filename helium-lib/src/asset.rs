use crate::{
    dao::Dao,
    entity_key::{self, AsEntityKey},
    keypair::{serde_opt_pubkey, serde_pubkey, Pubkey},
    kta,
    result::{DecodeError, Error, Result},
    settings::{DasClient, DasSearchAssetsParams, Settings},
};
use helium_anchor_gen::helium_entity_manager;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use solana_sdk::bs58;
use std::{collections::HashMap, result::Result as StdResult, str::FromStr};

pub async fn for_entity_key<E>(settings: &Settings, entity_key: &E) -> Result<Asset>
where
    E: AsEntityKey,
{
    let kta = kta::for_entity_key(entity_key).await?;
    for_kta(settings, &kta).await
}

pub async fn for_kta(
    settings: &Settings,
    kta: &helium_entity_manager::KeyToAssetV0,
) -> Result<Asset> {
    let jsonrpc = settings.mk_jsonrpc_client()?;
    let asset_responase: Asset = jsonrpc.get_asset(&kta.asset).await?;
    Ok(asset_responase)
}

pub async fn for_kta_with_proof(
    settings: &Settings,
    kta: &helium_entity_manager::KeyToAssetV0,
) -> Result<(Asset, AssetProof)> {
    let (asset, asset_proof) =
        futures::try_join!(for_kta(settings, kta), proof::get(settings, kta))?;
    Ok((asset, asset_proof))
}

pub async fn get_canopy_heights() -> Result<HashMap<Pubkey, usize>> {
    const KNOWN_CANOPY_HEIGHT_URL: &str = "https://shdw-drive.genesysgo.net/6tcnBSybPG7piEDShBcrVtYJDPSvGrDbVvXmXKpzBvWP/merkles.json";
    let client = reqwest::Client::new();
    let map: HashMap<String, usize> = client
        .get(KNOWN_CANOPY_HEIGHT_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    map.into_iter()
        .map(|(str, value)| {
            Pubkey::from_str(str.as_str())
                .map_err(|err| DecodeError::from(err).into())
                .map(|key| (key, value))
        })
        .try_collect()
}

pub mod proof {
    use super::*;

    pub async fn get(
        settings: &Settings,
        kta: &helium_entity_manager::KeyToAssetV0,
    ) -> Result<AssetProof> {
        let jsonrpc = settings.mk_jsonrpc_client()?;
        let asset_proof_response: AssetProof = jsonrpc.get_asset_proof(&kta.asset).await?;

        Ok(asset_proof_response)
    }

    pub async fn for_entity_key<E>(settings: &Settings, entity_key: &E) -> Result<AssetProof>
    where
        E: AsEntityKey,
    {
        let kta = kta::for_entity_key(entity_key).await?;
        get(settings, &kta).await
    }
}

pub async fn search(client: &DasClient, params: DasSearchAssetsParams) -> Result<AssetPage> {
    Ok(client.search_assets(params).await?)
}

pub async fn for_owner(
    settings: &Settings,
    creator: &Pubkey,
    owner: &Pubkey,
) -> Result<Vec<Asset>> {
    let mut params = DasSearchAssetsParams::for_owner(*owner, *creator);
    let mut results = vec![];
    let client = settings.mk_jsonrpc_client()?;
    loop {
        let page = search(&client, params.clone()).await.map_err(Error::from)?;
        if page.items.is_empty() {
            break;
        }
        results.extend(page.items);
        params.page += 1;
    }

    Ok(results)
}

#[derive(Deserialize, Serialize, Clone)]
pub struct AssetPage {
    pub total: u32,
    pub limit: u32,
    pub page: u32,
    pub items: Vec<Asset>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Asset {
    #[serde(with = "serde_pubkey")]
    pub id: Pubkey,
    pub compression: AssetCompression,
    pub creators: Vec<AssetCreator>,
    pub ownership: AssetOwnership,
    pub content: AssetContent,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetCreator {
    #[serde(with = "serde_pubkey")]
    address: Pubkey,
    share: u8,
    verified: bool,
}

pub type Hash = [u8; 32];

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetCompression {
    #[serde(with = "serde_hash")]
    pub data_hash: Hash,
    #[serde(with = "serde_hash")]
    pub creator_hash: Hash,
    pub leaf_id: u64,
    #[serde(with = "serde_pubkey")]
    pub tree: Pubkey,
}

impl AssetCompression {
    pub fn leaf_id(&self) -> StdResult<u32, DecodeError> {
        self.leaf_id.try_into().map_err(DecodeError::from)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetOwnership {
    #[serde(with = "serde_pubkey")]
    pub owner: Pubkey,
    #[serde(with = "serde_opt_pubkey")]
    pub delegate: Option<Pubkey>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AssetProof {
    pub proof: Vec<String>,
    #[serde(with = "serde_pubkey")]
    pub root: Pubkey,
    #[serde(with = "serde_pubkey")]
    pub tree_id: Pubkey,
}

impl Asset {
    pub fn kta_key(&self) -> Result<Pubkey> {
        if let Some(creator) = self.creators.get(1) {
            return Ok(creator.address);
        }
        let entity_key_str = self
            .content
            .json_uri
            .path()
            .strip_prefix('/')
            .map(ToString::to_string)
            .ok_or(DecodeError::other(format!(
                "missing entity key in \"{}\"",
                self.content.json_uri
            )))?;
        let key_serialization =
            if ["IOT OPS", "CARRIER"].contains(&self.content.metadata.symbol.as_str()) {
                helium_entity_manager::KeySerialization::UTF8
            } else {
                helium_entity_manager::KeySerialization::B58
            };
        let entity_key = entity_key::from_string(entity_key_str, key_serialization)?;
        let kta_key = Dao::Hnt.entity_key_to_kta_key(&entity_key);
        Ok(kta_key)
    }

    pub async fn get_kta(&self) -> Result<helium_entity_manager::KeyToAssetV0> {
        kta::get(&self.kta_key()?).await
    }
}

impl AssetProof {
    pub fn proof(
        &self,
        len: Option<usize>,
    ) -> Result<Vec<solana_program::instruction::AccountMeta>> {
        self.proof
            .iter()
            .take(len.unwrap_or(self.proof.len()))
            .map(|s| {
                Pubkey::from_str(s)
                    .map_err(DecodeError::from)
                    .map(|pubkey| solana_program::instruction::AccountMeta {
                        pubkey,
                        is_signer: false,
                        is_writable: false,
                    })
                    .map_err(Error::from)
            })
            .collect()
    }

    pub async fn proof_for_tree(
        &self,
        tree: &Pubkey,
    ) -> Result<Vec<solana_program::instruction::AccountMeta>> {
        let canopy_heights = get_canopy_heights().await?;
        let height = canopy_heights
            .get(tree)
            .ok_or_else(|| anchor_client::ClientError::AccountNotFound)?;
        self.proof(Some(*height))
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetContent {
    pub metadata: AssetMetadata,
    pub json_uri: url::Url,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetMetadata {
    #[serde(default)]
    pub attributes: Vec<AssetMetadataAttribute>,
    pub symbol: String,
}

impl AssetMetadata {
    pub fn get_attribute(&self, trait_type: &str) -> Option<&serde_json::Value> {
        self.attributes
            .iter()
            .filter(|entry| entry.trait_type == trait_type)
            .collect::<Vec<&AssetMetadataAttribute>>()
            .first()
            .map(|entry| &entry.value)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetMetadataAttribute {
    #[serde(default)]
    pub value: serde_json::Value,
    pub trait_type: String,
}

pub mod serde_hash {
    use super::*;
    use serde::de;

    pub fn serialize<S>(value: &Hash, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&bs58::encode(value).into_string())
    }

    pub fn deserialize<'de, D>(deser: D) -> std::result::Result<Hash, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let str = String::deserialize(deser)?;
        bs58::decode(&str)
            .into_vec()
            .map_err(|_| de::Error::custom("invalid hash"))?
            .as_slice()
            .try_into()
            .map_err(|_| de::Error::custom("invalid hash"))
    }
}
