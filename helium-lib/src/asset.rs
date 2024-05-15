use crate::{
    dao::Dao,
    entity_key::AsEntityKey,
    keypair::{serde_opt_pubkey, serde_pubkey, Keypair, Pubkey},
    result::{DecodeError, Error, Result},
    settings::{DasClient, DasSearchAssetsParams, Settings},
};
use helium_anchor_gen::helium_entity_manager;
use serde::{Deserialize, Serialize};
use solana_sdk::{bs58, signer::Signer};
use std::{ops::Deref, result::Result as StdResult, str::FromStr};

pub async fn account_for_entity_key<C: Clone + Deref<Target = impl Signer>, E>(
    client: &anchor_client::Client<C>,
    entity_key: &E,
) -> Result<helium_entity_manager::KeyToAssetV0>
where
    E: AsEntityKey,
{
    let program = client.program(helium_entity_manager::id())?;
    let asset_key = Dao::Hnt.key_to_asset_key(entity_key);
    let asset_account = program
        .account::<helium_entity_manager::KeyToAssetV0>(asset_key)
        .await?;
    Ok(asset_account)
}

pub async fn for_entity_key<E>(settings: &Settings, entity_key: &E) -> Result<Asset>
where
    E: AsEntityKey,
{
    let client = settings.mk_anchor_client(Keypair::void())?;
    let asset_account = account_for_entity_key(&client, entity_key).await?;
    get(settings, &asset_account).await
}

pub async fn get(
    settings: &Settings,
    asset_account: &helium_entity_manager::KeyToAssetV0,
) -> Result<Asset> {
    let jsonrpc = settings.mk_jsonrpc_client()?;
    let asset_responase: Asset = jsonrpc.get_asset(&asset_account.asset).await?;
    Ok(asset_responase)
}

pub mod proof {
    use super::*;

    pub async fn get(
        settings: &Settings,
        asset_account: &helium_entity_manager::KeyToAssetV0,
    ) -> Result<AssetProof> {
        let jsonrpc = settings.mk_jsonrpc_client()?;
        let asset_proof_response: AssetProof =
            jsonrpc.get_asset_proof(&asset_account.asset).await?;

        Ok(asset_proof_response)
    }

    pub async fn for_entity_key<E>(settings: &Settings, entity_key: &E) -> Result<AssetProof>
    where
        E: AsEntityKey,
    {
        let client = settings.mk_anchor_client(Keypair::void())?;
        let asset_account = account_for_entity_key(&client, entity_key).await?;
        get(settings, &asset_account).await
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
    pub ownership: AssetOwnership,
    pub content: AssetContent,
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
}

impl AssetProof {
    pub fn proof(&self) -> Result<Vec<solana_program::instruction::AccountMeta>> {
        self.proof
            .iter()
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
