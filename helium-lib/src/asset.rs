use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    bs58, bubblegum,
    client::{DasClient, DasSearchAssetsParams, SolanaRpcClient},
    dao::Dao,
    entity_key::{self, AsEntityKey},
    error::{DecodeError, Error},
    helium_entity_manager,
    keypair::{serde_opt_pubkey, serde_pubkey, Keypair, Pubkey},
    kta, message,
    priority_fee::{compute_budget_instruction, compute_price_instruction_for_accounts},
    programs::{SPL_NOOP_PROGRAM_ID, TOKEN_METADATA_PROGRAM_ID},
    solana_sdk::instruction::{AccountMeta, Instruction},
    spl_account_compression,
    transaction::{mk_transaction, VersionedTransaction},
    TransactionOpts,
};
use futures::{stream, StreamExt, TryStreamExt};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use solana_sdk::{signature::NullSigner, signer::Signer};
use std::{collections::HashMap, result::Result as StdResult, str::FromStr};

pub fn bubblegum_signer_key() -> Pubkey {
    Pubkey::find_program_address(&[b"collection_cpi"], &bubblegum::ID).0
}

pub fn collection_metadata_key(collection_key: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            collection_key.as_ref(),
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    )
    .0
}

pub fn collection_master_edition_key(collection_key: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            collection_key.as_ref(),
            b"edition",
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    )
    .0
}

pub fn merkle_tree_authority_key(merkle_tree: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[merkle_tree.as_ref()], &bubblegum::ID).0
}

pub fn shared_merkle_key(proof_size: u8) -> Pubkey {
    Pubkey::find_program_address(
        &[b"shared_merkle", &[proof_size]],
        &helium_entity_manager::ID,
    )
    .0
}

pub async fn for_entity_key<E, C: AsRef<DasClient>>(
    client: &C,
    entity_key: &E,
) -> Result<Asset, Error>
where
    E: AsEntityKey,
{
    let kta = kta::for_entity_key(entity_key).await?;
    for_kta(client, &kta).await
}

pub async fn for_entity_keys<E, C: AsRef<DasClient>>(
    client: &C,
    entity_keys: &[E],
) -> Result<Vec<Asset>, Error>
where
    E: AsEntityKey,
{
    let ktas = kta::for_entity_keys(entity_keys).await?;
    for_ktas(client, ktas.as_slice()).await
}

pub async fn for_kta<C: AsRef<DasClient>>(
    client: &C,
    kta: &helium_entity_manager::accounts::KeyToAssetV0,
) -> Result<Asset, Error> {
    get(client, &kta.asset).await
}

pub async fn for_ktas<C: AsRef<DasClient>>(
    client: &C,
    ktas: &[helium_entity_manager::accounts::KeyToAssetV0],
) -> Result<Vec<Asset>, Error> {
    let pubkeys = ktas.iter().map(|kta| kta.asset).collect_vec();
    get_many(client, &pubkeys).await
}

pub async fn for_kta_with_proof<C: AsRef<DasClient>>(
    client: &C,
    kta: &helium_entity_manager::accounts::KeyToAssetV0,
) -> Result<(Asset, AssetProof), Error> {
    get_with_proof(client, &kta.asset).await
}

pub async fn get<C: AsRef<DasClient>>(client: &C, pubkey: &Pubkey) -> Result<Asset, Error> {
    let asset_response: Asset = client.as_ref().get_asset(pubkey).await?;
    Ok(asset_response)
}

pub async fn get_many<C: AsRef<DasClient>>(
    client: &C,
    pubkeys: &[Pubkey],
) -> Result<Vec<Asset>, Error> {
    let assets = stream::iter(pubkeys.to_vec())
        .chunks(1000)
        .map(|key_chunk| async move { client.as_ref().get_asset_batch(key_chunk.as_slice()).await })
        .buffered(5)
        .try_collect::<Vec<Vec<Asset>>>()
        .await?
        .into_iter()
        .flatten()
        .collect_vec();
    Ok(assets)
}

pub async fn get_with_proof<C: AsRef<DasClient>>(
    client: &C,
    pubkey: &Pubkey,
) -> Result<(Asset, AssetProof), Error> {
    let (asset, asset_proof) = futures::try_join!(get(client, pubkey), proof::get(client, pubkey))?;
    Ok((asset, asset_proof))
}

pub fn merkle_tree_authority(merkle_tree: &Pubkey) -> Pubkey {
    let (tree_authority, _ta_bump) =
        Pubkey::find_program_address(&[merkle_tree.as_ref()], &bubblegum::ID);
    tree_authority
}

pub fn bubblegum_signer() -> Pubkey {
    let (bubblegum_signer, _bump) =
        Pubkey::find_program_address(&[b"collection_cpi"], &bubblegum::ID);
    bubblegum_signer
}

pub mod canopy {
    use super::*;

    async fn get_heights() -> Result<HashMap<Pubkey, usize>, Error> {
        const KNOWN_CANOPY_HEIGHT_URL: &str = "https://entities.nft.helium.io/v2/merkles";
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

    pub async fn height_for_tree<C: AsRef<SolanaRpcClient>>(
        _client: &C,
        tree: &Pubkey,
    ) -> Result<usize, Error> {
        get_heights()
            .await?
            .get(tree)
            .copied()
            .ok_or_else(Error::account_not_found)
    }
}

pub mod proof {
    use super::*;

    pub async fn get<C: AsRef<DasClient>>(
        client: &C,
        pubkey: &Pubkey,
    ) -> Result<AssetProof, Error> {
        let asset_proof_response: AssetProof = client.as_ref().get_asset_proof(pubkey).await?;
        Ok(asset_proof_response)
    }

    pub async fn get_many<C: AsRef<DasClient>>(
        client: &C,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<AssetProof>, Error> {
        let proofs = stream::iter(pubkeys.to_vec())
            .chunks(1000)
            .map(|key_chunk| async move {
                client
                    .as_ref()
                    .get_asset_proof_batch(key_chunk.as_slice())
                    .await
            })
            .buffered(5)
            .try_collect::<Vec<Vec<AssetProof>>>()
            .await?
            .into_iter()
            .flatten()
            .collect_vec();
        Ok(proofs)
    }
}

pub async fn search<C: AsRef<DasClient>>(
    client: &C,
    params: DasSearchAssetsParams,
) -> Result<AssetPage, Error> {
    Ok(client.as_ref().search_assets(params).await?)
}

pub async fn for_owner<C: AsRef<DasClient>>(
    client: &C,
    creator: &Pubkey,
    owner: &Pubkey,
) -> Result<Vec<Asset>, Error> {
    let mut params = DasSearchAssetsParams::for_owner(*owner, *creator);
    // Set to maximum documented limit
    params.limit = 1000;
    let mut results = vec![];
    loop {
        let page = client
            .as_ref()
            .search_assets(params.clone())
            .await
            .map_err(Error::from)?;
        let fetch_count = page.items.len();
        results.extend(page.items);
        if fetch_count < params.limit as usize {
            break;
        }
        params.page += 1;
    }

    Ok(results)
}

pub fn transfer_instruction(
    recipient: &Pubkey,
    asset: &Asset,
    asset_proof: &AssetProof,
    remaining_accounts: &[AccountMeta],
) -> Result<Instruction, Error> {
    use bubblegum::client::{
        accounts::Transfer as TransferAccounts, args::Transfer as TransferArgs,
    };
    let leaf_delegate = asset.ownership.delegate.unwrap_or(asset.ownership.owner);
    let mut accounts = TransferAccounts {
        leaf_owner: asset.ownership.owner,
        leaf_delegate,
        new_leaf_owner: *recipient,
        tree_authority: merkle_tree_authority(&asset_proof.tree_id),
        merkle_tree: asset_proof.tree_id,
        log_wrapper: SPL_NOOP_PROGRAM_ID,
        compression_program: spl_account_compression::ID,
        system_program: solana_sdk::system_program::id(),
    }
    .to_account_metas(None);
    accounts.extend_from_slice(remaining_accounts);

    let data = TransferArgs {
        creator_hash: asset.compression.creator_hash,
        root: asset_proof.root.to_bytes(),
        data_hash: asset.compression.data_hash,
        index: asset.compression.leaf_id()?,
        nonce: asset.compression.leaf_id,
    }
    .data();

    let ix = Instruction {
        program_id: bubblegum::ID,
        accounts,
        data,
    };
    Ok(ix)
}

/// Get an unsigned transaction for an asset transfer
///
/// The asset is transferred from the owner to the given recipient
/// Note that the owner is currently expected to sign this transaction and pay for
/// transaction fees.
pub async fn transfer_transaction<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    pubkey: &Pubkey,
    recipient: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (asset, asset_proof) = get_with_proof(client, pubkey).await?;
    let remaining_accounts = asset_proof
        .proof_for_tree(client, &asset_proof.tree_id)
        .await?;
    let ix = transfer_instruction(recipient, &asset, &asset_proof, &remaining_accounts)?;

    let ixs = &[
        compute_budget_instruction(200_000),
        compute_price_instruction_for_accounts(client, &ix.accounts, opts.fee_range()).await?,
        ix,
    ];

    let (msg, block_height) =
        message::mk_message(client, ixs, &opts.lut_addresses, &asset.ownership.owner).await?;
    let txn = mk_transaction(msg, &[&NullSigner::new(&asset.ownership.owner)])?;
    Ok((txn, block_height))
}

pub async fn transfer<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    pubkey: &Pubkey,
    recipient: &Pubkey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (mut txn, block_height) = transfer_transaction(client, pubkey, recipient, opts).await?;
    let message_data = txn.message.serialize();
    let signature = keypair.try_sign_message(&message_data)?;
    txn.signatures[0] = signature;
    Ok((txn, block_height))
}

/// Get an unsigned burn transaction for an asset
pub async fn burn_message<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    pubkey: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    use bubblegum::client::{accounts::Burn as BurnAccounts, args::Burn as BurnArgs};
    let (asset, asset_proof) = get_with_proof(client, pubkey).await?;

    let leaf_delegate = asset.ownership.delegate.unwrap_or(asset.ownership.owner);
    let remaining_accounts = asset_proof
        .proof_for_tree(client, &asset_proof.tree_id)
        .await?;

    let burn = BurnAccounts {
        leaf_owner: asset.ownership.owner,
        leaf_delegate,
        merkle_tree: asset_proof.tree_id,
        tree_authority: merkle_tree_authority(&asset_proof.tree_id),
        log_wrapper: SPL_NOOP_PROGRAM_ID,
        compression_program: spl_account_compression::ID,
        system_program: solana_sdk::system_program::id(),
    };

    let args = BurnArgs {
        creator_hash: asset.compression.creator_hash,
        root: asset_proof.root.to_bytes(),
        data_hash: asset.compression.data_hash,
        index: asset.compression.leaf_id()?,
        nonce: asset.compression.leaf_id,
    };

    let ix = Instruction {
        program_id: bubblegum::ID,
        accounts: burn.to_account_metas(None),
        data: args.data(),
    };

    // burn.instruction_with_remaining_accounts(args, &remaining_accounts);

    let mut priority_fee_accounts = ix.accounts.clone();
    priority_fee_accounts.extend_from_slice(&remaining_accounts);

    let ixs = &[
        compute_budget_instruction(100_000),
        compute_price_instruction_for_accounts(client, &priority_fee_accounts, opts.fee_range())
            .await?,
        ix,
    ];

    message::mk_message(client, ixs, &opts.lut_addresses, &asset.ownership.owner).await
}

pub async fn burn<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    pubkey: &Pubkey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (msg, block_height) = burn_message(client, pubkey, opts).await?;
    let txn = mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
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
    pub grouping: Vec<AssetGroup>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub burnt: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AssetCreator {
    #[serde(with = "serde_pubkey")]
    pub address: Pubkey,
    pub share: u8,
    pub verified: bool,
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
pub struct AssetGroup {
    pub group_key: String,
    #[serde(with = "serde_pubkey")]
    pub group_value: Pubkey,
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
    pub fn kta_key(&self) -> Result<Pubkey, Error> {
        if let Some(creator) = self.creators.get(1) {
            return Ok(creator.address);
        }
        let Some((_, entity_key_str)) = self.content.json_uri.path().rsplit_once('/') else {
            return Err(DecodeError::other(format!(
                "missing entity key in \"{}\"",
                self.content.json_uri
            ))
            .into());
        };
        let key_serialization =
            if ["IOT OPS", "CARRIER"].contains(&self.content.metadata.symbol.as_str()) {
                helium_entity_manager::types::KeySerialization::UTF8
            } else {
                helium_entity_manager::types::KeySerialization::B58
            };
        let entity_key = entity_key::from_str(entity_key_str, key_serialization)?;
        let kta_key = Dao::Hnt.entity_key_to_kta_key(&entity_key);
        Ok(kta_key)
    }

    pub async fn get_kta(&self) -> Result<helium_entity_manager::accounts::KeyToAssetV0, Error> {
        kta::get(&self.kta_key()?).await
    }

    pub fn is_symbol(&self, symbol: &str) -> bool {
        self.content.metadata.symbol == symbol
    }
}

impl AssetProof {
    pub fn proof(
        &self,
        len: Option<usize>,
    ) -> Result<Vec<solana_program::instruction::AccountMeta>, Error> {
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

    pub async fn proof_for_tree<C: AsRef<SolanaRpcClient>>(
        &self,
        client: &C,
        tree: &Pubkey,
    ) -> Result<Vec<AccountMeta>, Error> {
        let height = canopy::height_for_tree(client, tree).await?;
        self.proof(Some(height))
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
    pub name: String,
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
