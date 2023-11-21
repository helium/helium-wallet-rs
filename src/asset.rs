use crate::{
    dao::Dao,
    keypair::{serde_pubkey, Pubkey},
    result::{Context, Error, Result},
    settings::Settings,
};
use anchor_client::{self, solana_sdk::signer::Signer};
use serde::Deserialize;
use serde_json::json;
use std::{ops::Deref, str::FromStr};

pub fn account_for_entity_key<C: Clone + Deref<Target = impl Signer>>(
    client: &anchor_client::Client<C>,
    entity_key: &[u8],
) -> Result<helium_entity_manager::KeyToAssetV0> {
    let program = client.program(helium_entity_manager::id())?;
    let asset_key = Dao::Hnt.key_to_asset(entity_key);
    let asset_account = program.account::<helium_entity_manager::KeyToAssetV0>(asset_key)?;
    Ok(asset_account)
}

pub fn get(
    settings: &Settings,
    asset_account: &helium_entity_manager::KeyToAssetV0,
) -> Result<Asset> {
    let jsonrpc = settings.mk_jsonrpc_client()?;
    let asset_responase: Asset = jsonrpc
        .call(
            "getAsset",
            &[jsonrpc::arg(json!({
                "id": asset_account.asset.to_string()
            }))],
        )
        .context("while getting asset")?;
    Ok(asset_responase)
}

pub fn get_proof(
    settings: &Settings,
    asset_account: &helium_entity_manager::KeyToAssetV0,
) -> Result<AsssetProof> {
    let jsonrpc = settings.mk_jsonrpc_client()?;
    let asset_proof_response: AsssetProof = jsonrpc
        .call(
            "getAssetProof",
            &[jsonrpc::arg(json!({
                "id": asset_account.asset.to_string()
            }))],
        )
        .context("while getting asset proof")?;
    Ok(asset_proof_response)
}

pub fn get_assets(settings: &Settings, creator: &Pubkey, owner: &Pubkey) -> Result<Vec<Asset>> {
    let mut params = json!({
        "creatorVerified": true,
        "creatorAddress": creator.to_string(),
        "ownerAddress": owner.to_string(),
    });
    let mut page = 1;
    let mut results = vec![];
    let client = settings.mk_jsonrpc_client()?;
    loop {
        params["page"] = page.into();
        let page_result: AssetPage = client.call("searchAssets", &[jsonrpc::arg(&params)])?;
        if page_result.items.is_empty() {
            break;
        }
        results.extend(page_result.items);
        page += 1;
    }

    Ok(results)
}

#[derive(Deserialize)]
struct AssetPage {
    items: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
pub struct Asset {
    #[serde(with = "serde_pubkey")]
    pub id: Pubkey,
    pub compression: AssetCompression,
    pub ownership: AssetOwnership,
    pub content: AssetContent,
}

pub type Hash = [u8; 32];

#[derive(Debug, Deserialize)]
pub struct AssetCompression {
    #[serde(with = "serde_hash")]
    pub data_hash: Hash,
    #[serde(with = "serde_hash")]
    pub creator_hash: Hash,
    pub leaf_id: u64,
    #[serde(with = "serde_pubkey")]
    pub tree: Pubkey,
}

#[derive(Debug, Deserialize)]
pub struct AssetOwnership {
    #[serde(with = "serde_pubkey")]
    pub owner: Pubkey,
}

#[derive(Debug, Deserialize)]
pub struct AsssetProof {
    pub proof: Vec<String>,
    #[serde(with = "serde_pubkey")]
    pub root: Pubkey,
}

impl AsssetProof {
    pub fn proof(&self) -> Result<Vec<solana_program::instruction::AccountMeta>> {
        self.proof
            .iter()
            .map(|s| {
                Pubkey::from_str(s).map_err(Error::from).map(|pubkey| {
                    solana_program::instruction::AccountMeta {
                        pubkey,
                        is_signer: false,
                        is_writable: false,
                    }
                })
            })
            .collect()
    }
}

#[derive(Debug, Deserialize)]
pub struct AssetContent {
    pub metadata: AssetMetadata,
}

#[derive(Debug, Deserialize)]
pub struct AssetMetadata {
    pub attributes: Vec<AssetMetadataAttribute>,
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

#[derive(Debug, Deserialize)]
pub struct AssetMetadataAttribute {
    pub value: serde_json::Value,
    pub trait_type: String,
}

pub mod serde_hash {
    use super::*;
    use serde::de::{self, Deserialize};

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
