use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    anchor_spl, asset, bs58,
    client::{DasClient, DasSearchAssetsParams, GetAnchorAccount, SolanaRpcClient},
    dao::{Dao, SubDao},
    data_credits,
    error::{DecodeError, EncodeError, Error},
    helium_entity_manager, is_zero,
    keypair::{pubkey, serde_pubkey, Keypair, Pubkey},
    kta, onboarding,
    priority_fee::{self, compute_budget_instruction, compute_price_instruction_for_accounts},
    programs::{SPL_ACCOUNT_COMPRESSION_PROGRAM_ID, SPL_NOOP_PROGRAM_ID},
    solana_sdk::{
        instruction::AccountMeta, instruction::Instruction, signer::Signer,
        transaction::Transaction,
    },
    token::Token,
};
use angry_purple_tiger::AnimalName;
use chrono::Utc;
use futures::{
    stream::{self, StreamExt, TryStreamExt},
    TryFutureExt,
};
use itertools::Itertools;
use rust_decimal::prelude::*;
use serde::Serialize;
use std::{collections::HashMap, hash::Hash, str::FromStr};

pub mod dataonly;
pub mod info;

pub const HOTSPOT_CREATOR: Pubkey = pubkey!("Fv5hf1Fg58htfC7YEXKNEfkpuogUUQDDTLgjGWxxv48H");
pub const ECC_VERIFIER: Pubkey = pubkey!("eccSAJM3tq7nQSpQTm8roxv4FPoipCkMsGizW2KBhqZ");

pub fn entity_key_from_kta(
    kta: &helium_entity_manager::KeyToAssetV0,
) -> Result<helium_crypto::PublicKey, Error> {
    let key_str = match kta.key_serialization {
        helium_entity_manager::KeySerialization::B58 => bs58::encode(&kta.entity_key).into_string(),
        helium_entity_manager::KeySerialization::UTF8 => String::from_utf8(kta.entity_key.to_vec())
            .map_err(|_| DecodeError::other("invalid entity key string"))?,
    };
    Ok(helium_crypto::PublicKey::from_str(&key_str)?)
}

pub async fn for_owner<C: AsRef<DasClient>>(
    client: &C,
    owner: &Pubkey,
) -> Result<Vec<Hotspot>, Error> {
    let assets = asset::for_owner(client, &HOTSPOT_CREATOR, owner).await?;
    let hotspot_assets = assets
        .into_iter()
        .filter(|asset| asset.is_symbol("HOTSPOT"));
    stream::iter(hotspot_assets)
        .map(|asset| async move { Hotspot::from_asset(asset).await })
        .buffered(5)
        .try_collect::<Vec<Hotspot>>()
        .await
}

pub async fn search<C: AsRef<DasClient>>(
    client: &C,
    params: DasSearchAssetsParams,
) -> Result<HotspotPage, Error> {
    asset::search(client, params)
        .map_ok(|mut asset_page| {
            asset_page.items.retain(|asset| asset.is_symbol("HOTSPOT"));
            asset_page
        })
        .and_then(HotspotPage::from_asset_page)
        .await
}
pub fn name(hotspot_key: &helium_crypto::PublicKey) -> String {
    hotspot_key
        .to_string()
        .parse::<AnimalName>()
        // can unwrap safely
        .unwrap()
        .to_string()
}

pub async fn get<C: AsRef<DasClient>>(
    client: &C,
    hotspot_key: &helium_crypto::PublicKey,
) -> Result<Hotspot, Error> {
    let kta = kta::for_entity_key(hotspot_key).await?;
    let asset = asset::for_kta(client, &kta).await?;
    Hotspot::from_asset(asset).await
}

pub async fn get_with_info<C: AsRef<DasClient> + GetAnchorAccount>(
    client: &C,
    subdaos: &[SubDao],
    hotspot_key: &helium_crypto::PublicKey,
) -> Result<Hotspot, Error> {
    let (mut hotspot, info) = futures::try_join!(
        get(client, hotspot_key),
        info::for_entity_key(client, subdaos, hotspot_key)
    )?;
    if !info.is_empty() {
        hotspot.info = Some(info);
    }
    Ok(hotspot)
}

pub async fn direct_update<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot: &helium_crypto::PublicKey,
    update: HotspotInfoUpdate,
    keypair: &Keypair,
) -> Result<Transaction, Error> {
    fn mk_accounts(
        subdao: SubDao,
        kta: &helium_entity_manager::KeyToAssetV0,
        asset: &asset::Asset,
        owner: &Pubkey,
    ) -> Vec<AccountMeta> {
        use helium_entity_manager::accounts::{UpdateIotInfoV0, UpdateMobileInfoV0};
        macro_rules! mk_update_info {
            ($name:ident, $info:ident) => {
                $name {
                    bubblegum_program: mpl_bubblegum::ID,
                    payer: owner.to_owned(),
                    dc_fee_payer: owner.to_owned(),
                    $info: subdao.info_key(&kta.entity_key),
                    hotspot_owner: owner.to_owned(),
                    merkle_tree: asset.compression.tree,
                    tree_authority: Dao::Hnt.merkle_tree_authority(&asset.compression.tree),
                    dc_burner: Token::Dc.associated_token_adress(owner),
                    rewardable_entity_config: subdao.rewardable_entity_config_key(),
                    dao: Dao::Hnt.key(),
                    sub_dao: subdao.key(),
                    dc_mint: *Token::Dc.mint(),
                    dc: SubDao::dc_key(),
                    compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                    data_credits_program: data_credits::id(),
                    token_program: anchor_spl::token::ID,
                    associated_token_program: spl_associated_token_account::id(),
                    system_program: solana_sdk::system_program::id(),
                }
                .to_account_metas(None)
            };
        }
        match subdao {
            SubDao::Iot => mk_update_info!(UpdateIotInfoV0, iot_info),
            SubDao::Mobile => mk_update_info!(UpdateMobileInfoV0, mobile_info),
        }
    }

    let kta = kta::for_entity_key(hotspot).await?;
    let (asset, asset_proof) = asset::for_kta_with_proof(&client, &kta).await?;
    let mut accounts = mk_accounts(update.subdao(), &kta, &asset, &keypair.pubkey());
    accounts.extend_from_slice(&asset_proof.proof(Some(3))?);

    let update_ix = Instruction {
        program_id: helium_entity_manager::id(),
        accounts: accounts.to_account_metas(None),
        data: helium_entity_manager::instruction::UpdateIotInfoV0 {
            _args: helium_entity_manager::UpdateIotInfoArgsV0 {
                root: asset_proof.root.to_bytes(),
                data_hash: asset.compression.data_hash,
                creator_hash: asset.compression.creator_hash,
                index: asset.compression.leaf_id()?,
                elevation: *update.elevation(),
                gain: update.gain_i32(),
                location: update.location_u64(),
            },
        }
        .data(),
    };

    let ixs = &[
        priority_fee::compute_budget_instruction(200_000),
        priority_fee::compute_price_instruction_for_accounts(client, &accounts).await?,
        update_ix,
    ];

    let recent_blockhash = AsRef::<SolanaRpcClient>::as_ref(client)
        .get_latest_blockhash()
        .await?;
    let tx = Transaction::new_signed_with_payer(
        ixs,
        Some(&keypair.pubkey()),
        &[keypair],
        recent_blockhash,
    );
    Ok(tx)
}

pub async fn update<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    onboarding_server: Option<String>,
    hotspot: &helium_crypto::PublicKey,
    update: HotspotInfoUpdate,
    keypair: &Keypair,
) -> Result<Transaction, Error> {
    let public_key = keypair.pubkey();
    if let Some(server) = onboarding_server {
        let onboarding_client = onboarding::Client::new(&server);
        let mut tx = onboarding_client
            .get_update_txn(hotspot, &public_key, update)
            .await?;
        tx.try_partial_sign(&[keypair], tx.message.recent_blockhash)?;
        return Ok(tx);
    };
    let tx = direct_update(client, hotspot, update, keypair).await?;
    Ok(tx)
}

/// Get an unsigned transaction for a hotspot transfer.
///
/// The hotspot is transferred from the owner of the hotspot to the given recipient
/// Note that the owner is currently expected to sign this transaction and pay for
/// transaction fees.
pub async fn transfer_transaction<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot_key: &helium_crypto::PublicKey,
    recipient: &Pubkey,
) -> Result<Transaction, Error> {
    let kta = kta::for_entity_key(hotspot_key).await?;
    let (asset, asset_proof) = asset::for_kta_with_proof(client, &kta).await?;

    let leaf_delegate = asset.ownership.delegate.unwrap_or(asset.ownership.owner);
    let merkle_tree = asset_proof.tree_id;
    let remaining_accounts = asset_proof.proof_for_tree(client, &merkle_tree).await?;

    let transfer = mpl_bubblegum::instructions::Transfer {
        leaf_owner: (asset.ownership.owner, false),
        leaf_delegate: (leaf_delegate, false),
        new_leaf_owner: *recipient,
        tree_config: mpl_bubblegum::accounts::TreeConfig::find_pda(&merkle_tree).0,
        merkle_tree,
        log_wrapper: SPL_NOOP_PROGRAM_ID,
        compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        system_program: solana_sdk::system_program::id(),
    };
    let args = mpl_bubblegum::instructions::TransferInstructionArgs {
        creator_hash: asset.compression.creator_hash,
        root: asset_proof.root.to_bytes(),
        data_hash: asset.compression.data_hash,
        index: asset.compression.leaf_id()?,
        nonce: asset.compression.leaf_id,
    };

    let transfer_ix = transfer.instruction_with_remaining_accounts(args, &remaining_accounts);
    let mut priority_fee_accounts = transfer_ix.accounts.clone();
    priority_fee_accounts.extend_from_slice(&remaining_accounts);

    let ixs = &[
        compute_budget_instruction(200_000),
        compute_price_instruction_for_accounts(client, &priority_fee_accounts).await?,
        transfer_ix,
    ];

    let tx = Transaction::new_with_payer(ixs, Some(&asset.ownership.owner));
    Ok(tx)
}

pub async fn transfer<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot_key: &helium_crypto::PublicKey,
    recipient: &Pubkey,
    keypair: &Keypair,
) -> Result<Transaction, Error> {
    let mut tx = transfer_transaction(client, hotspot_key, recipient).await?;
    let blockhash = AsRef::<SolanaRpcClient>::as_ref(client)
        .get_latest_blockhash()
        .await?;
    tx.try_sign(&[keypair], blockhash)?;

    Ok(tx)
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "kebab-case")]
pub enum HotspotMode {
    Full,
    #[default]
    DataOnly,
}

impl From<bool> for HotspotMode {
    fn from(value: bool) -> Self {
        if value {
            Self::Full
        } else {
            Self::DataOnly
        }
    }
}

impl std::fmt::Display for HotspotMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Self::Full => "full",
            Self::DataOnly => "data-only",
        };
        f.write_str(str)
    }
}

impl std::str::FromStr for HotspotMode {
    type Err = DecodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full" => Ok(Self::Full),
            "data-only" => Ok(Self::DataOnly),
            _ => Err(DecodeError::other("invalid hotspot mode")),
        }
    }
}

#[derive(Serialize, Clone)]
pub struct HotspotPage {
    pub total: u32,
    pub limit: u32,
    pub page: u32,
    pub items: Vec<Hotspot>,
}

impl HotspotPage {
    pub async fn from_asset_page(asset_page: asset::AssetPage) -> Result<Self, Error> {
        let kta_keys: Vec<Pubkey> = asset_page
            .items
            .iter()
            .map(asset::Asset::kta_key)
            .try_collect()?;
        let ktas = kta::get_many(&kta_keys).await?;
        let items: Vec<Hotspot> = ktas
            .into_iter()
            .zip(asset_page.items)
            .map(|(kta, asset)| Hotspot::from_asset_with_kta(kta, asset))
            .try_collect()?;
        Ok(Self {
            total: asset_page.total,
            limit: asset_page.limit,
            page: asset_page.page,
            items,
        })
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Hotspot {
    pub key: helium_crypto::PublicKey,
    #[serde(with = "serde_pubkey")]
    pub asset: Pubkey,
    pub name: String,
    #[serde(with = "serde_pubkey")]
    pub owner: Pubkey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<HashMap<SubDao, HotspotInfo>>,
}

impl Hotspot {
    pub async fn from_asset(asset: asset::Asset) -> Result<Self, Error> {
        let kta = asset.get_kta().await?;
        Self::from_asset_with_kta(kta, asset)
    }

    pub fn from_asset_with_kta(
        kta: helium_entity_manager::KeyToAssetV0,
        asset: asset::Asset,
    ) -> Result<Self, Error> {
        let entity_key = entity_key_from_kta(&kta)?;
        Ok(Self {
            asset: kta.asset,
            name: name(&entity_key),
            key: entity_key,
            owner: asset.ownership.owner,
            info: None,
        })
    }
}

#[derive(Serialize, Debug, Clone, Copy)]
pub struct HotspotGeo {
    pub lat: f64,
    pub lng: f64,
}

impl From<h3o::CellIndex> for HotspotGeo {
    fn from(value: h3o::CellIndex) -> Self {
        let lat_lng = h3o::LatLng::from(value);
        Self {
            lat: lat_lng.lat(),
            lng: lat_lng.lng(),
        }
    }
}

#[derive(Serialize, Debug, Clone, Copy)]
pub struct HotspotLocation {
    #[serde(with = "serde_cell_index")]
    pub location: h3o::CellIndex,
    pub geo: HotspotGeo,
}

impl std::hash::Hash for HotspotLocation {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.location.hash(state)
    }
}

impl From<h3o::CellIndex> for HotspotLocation {
    fn from(value: h3o::CellIndex) -> Self {
        Self {
            location: value,
            geo: HotspotGeo::from(value),
        }
    }
}

impl From<HotspotLocation> for u64 {
    fn from(value: HotspotLocation) -> Self {
        value.location.into()
    }
}

impl TryFrom<u64> for HotspotLocation {
    type Error = h3o::error::InvalidCellIndex;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        h3o::CellIndex::try_from(value).map(Into::into)
    }
}

impl FromStr for HotspotLocation {
    type Err = h3o::error::InvalidCellIndex;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<h3o::CellIndex>().map(Into::into)
    }
}

impl std::fmt::Display for HotspotLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.location.fmt(f)
    }
}

impl HotspotLocation {
    pub fn from_maybe<T: TryInto<HotspotLocation>>(value: Option<T>) -> Option<Self> {
        value.and_then(|v| TryInto::try_into(v).ok())
    }
}

pub mod serde_cell_index {
    use serde::de::{self, Deserialize};
    use std::str::FromStr;

    pub fn serialize<S>(
        value: &h3o::CellIndex,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deser: D) -> std::result::Result<h3o::CellIndex, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let str = String::deserialize(deser)?;
        h3o::CellIndex::from_str(&str).map_err(|_| de::Error::custom("invalid h3 index"))
    }
}

#[derive(Debug, Serialize, Clone, Hash)]
#[serde(rename_all = "lowercase", untagged)]
pub enum HotspotInfo {
    Iot {
        mode: HotspotMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        gain: Option<Decimal>,
        #[serde(skip_serializing_if = "Option::is_none")]
        elevation: Option<i32>,
        #[serde(flatten)]
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<HotspotLocation>,
        #[serde(skip_serializing_if = "is_zero")]
        location_asserts: u16,
    },
    Mobile {
        mode: HotspotMode,
        #[serde(flatten)]
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<HotspotLocation>,
        #[serde(skip_serializing_if = "is_zero")]
        location_asserts: u16,
        device_type: MobileDeviceType,
    },
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub struct CommittedHotspotInfoUpdate {
    pub block: u64,
    pub timestamp: chrono::DateTime<Utc>,
    pub signature: String,
    #[serde(with = "serde_pubkey")]
    pub info_key: Pubkey,
    pub update: HotspotInfoUpdate,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase", untagged)]
pub enum HotspotInfoUpdate {
    Iot {
        #[serde(skip_serializing_if = "Option::is_none")]
        gain: Option<Decimal>,
        #[serde(skip_serializing_if = "Option::is_none")]
        elevation: Option<i32>,
        #[serde(flatten)]
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<HotspotLocation>,
    },
    Mobile {
        #[serde(flatten)]
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<HotspotLocation>,
    },
}

impl HotspotInfoUpdate {
    pub fn subdao(&self) -> SubDao {
        match self {
            Self::Iot { .. } => SubDao::Iot,
            Self::Mobile { .. } => SubDao::Mobile,
        }
    }

    pub fn for_subdao(subdao: SubDao) -> Self {
        match subdao {
            SubDao::Iot => Self::Iot {
                gain: None,
                elevation: None,
                location: None,
            },
            SubDao::Mobile => Self::Mobile { location: None },
        }
    }

    pub fn location(&self) -> &Option<HotspotLocation> {
        match self {
            Self::Iot { location, .. } => location,
            Self::Mobile { location, .. } => location,
        }
    }

    pub fn set_location(mut self, new_location: Option<h3o::CellIndex>) -> Self {
        let hotspot_location = new_location.map(HotspotLocation::from);
        match self {
            Self::Iot {
                ref mut location, ..
            } => *location = hotspot_location,
            Self::Mobile {
                ref mut location, ..
            } => *location = hotspot_location,
        }
        self
    }

    pub fn set_geo(self, lat: Option<f64>, lon: Option<f64>) -> Result<Self, EncodeError> {
        let location: Option<h3o::CellIndex> = match (lat, lon) {
            (Some(lat), Some(lon)) => Some(
                h3o::LatLng::new(lat, lon)
                    .map_err(EncodeError::from)?
                    .to_cell(h3o::Resolution::Twelve),
            ),
            (None, None) => None,
            _ => return Err(EncodeError::other("Both lat and lon must be specified")),
        };
        Ok(self.set_location(location))
    }

    pub fn location_u64(&self) -> Option<u64> {
        self.location().map(Into::into)
    }

    pub fn set_elevation(mut self, new_elevation: Option<i32>) -> Self {
        if let Self::Iot {
            ref mut elevation, ..
        } = self
        {
            *elevation = new_elevation
        };
        self
    }

    pub fn elevation(&self) -> &Option<i32> {
        match self {
            Self::Iot { elevation, .. } => elevation,
            Self::Mobile { .. } => &None,
        }
    }

    pub fn gain_i32(&self) -> Option<i32> {
        self.gain().and_then(|gain| {
            f32::try_from(gain)
                .map(|fgain| (fgain * 10.0).trunc() as i32)
                .ok()
        })
    }

    pub fn gain(&self) -> &Option<Decimal> {
        match self {
            Self::Iot { gain, .. } => gain,
            Self::Mobile { .. } => &None,
        }
    }

    pub fn set_gain(mut self, new_gain: Option<f64>) -> Self {
        match self {
            Self::Iot { ref mut gain, .. } => {
                *gain = new_gain
                    .and_then(|gain| Decimal::from_f64(gain).map(|dec| dec.trunc_with_scale(1)))
            }
            Self::Mobile { .. } => (),
        }
        self
    }
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "snake_case")]
pub enum MobileDeviceType {
    #[default]
    Cbrs,
    WifiIndoor,
    WifiOutdoor,
    WifiDataOnly,
}

impl std::fmt::Display for MobileDeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Self::Cbrs => "cbrs",
            Self::WifiIndoor => "wifi_indoor",
            Self::WifiOutdoor => "wifi_outdoor",
            Self::WifiDataOnly => "wifi_data_only",
        };
        f.write_str(str)
    }
}

impl std::str::FromStr for MobileDeviceType {
    type Err = DecodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let value = match s {
            "cbrs" => Self::Cbrs,
            "wifi_indoor" => Self::WifiIndoor,
            "wifi_outdoor" => Self::WifiOutdoor,
            "wifi_data_only" => Self::WifiDataOnly,
            _ => return Err(DecodeError::other("invalid mobile device type")),
        };
        Ok(value)
    }
}

impl From<helium_entity_manager::MobileDeviceTypeV0> for MobileDeviceType {
    fn from(value: helium_entity_manager::MobileDeviceTypeV0) -> Self {
        match value {
            helium_entity_manager::MobileDeviceTypeV0::Cbrs => Self::Cbrs,
            helium_entity_manager::MobileDeviceTypeV0::WifiIndoor => Self::WifiIndoor,
            helium_entity_manager::MobileDeviceTypeV0::WifiOutdoor => Self::WifiOutdoor,
            helium_entity_manager::MobileDeviceTypeV0::WifiDataOnly => Self::WifiDataOnly,
        }
    }
}

impl HotspotInfo {
    pub fn from_maybe<T: Into<Self>>(value: Option<T>) -> Option<Self> {
        value.map(Into::into)
    }

    pub fn location(&self) -> &Option<HotspotLocation> {
        match self {
            Self::Iot { location, .. } => location,
            Self::Mobile { location, .. } => location,
        }
    }

    pub fn location_u64(&self) -> Option<u64> {
        self.location().map(Into::into)
    }

    pub fn elevation(&self) -> &Option<i32> {
        match self {
            Self::Iot { elevation, .. } => elevation,
            Self::Mobile { .. } => &None,
        }
    }

    pub fn gain_i32(&self) -> Option<i32> {
        self.gain().and_then(|gain| {
            f32::try_from(gain)
                .map(|fgain| (fgain * 10.0).trunc() as i32)
                .ok()
        })
    }

    pub fn gain(&self) -> &Option<Decimal> {
        match self {
            Self::Iot { gain, .. } => gain,
            Self::Mobile { .. } => &None,
        }
    }

    pub fn mode(&self) -> HotspotMode {
        match self {
            Self::Iot { mode, .. } => *mode,
            Self::Mobile { mode, .. } => *mode,
        }
    }

    pub fn mobile_device_type(&self) -> Option<MobileDeviceType> {
        match self {
            Self::Iot { .. } => None,
            Self::Mobile { device_type, .. } => Some(*device_type),
        }
    }
}

impl From<helium_entity_manager::IotHotspotInfoV0> for HotspotInfo {
    fn from(value: helium_entity_manager::IotHotspotInfoV0) -> Self {
        Self::Iot {
            mode: value.is_full_hotspot.into(),
            gain: value.gain.map(|gain| Decimal::new(gain.into(), 1)),
            elevation: value.elevation,
            location: HotspotLocation::from_maybe(value.location),
            location_asserts: value.num_location_asserts,
        }
    }
}

impl From<helium_entity_manager::MobileHotspotInfoV0> for HotspotInfo {
    fn from(value: helium_entity_manager::MobileHotspotInfoV0) -> Self {
        Self::Mobile {
            mode: value.is_full_hotspot.into(),
            location: HotspotLocation::from_maybe(value.location),
            location_asserts: value.num_location_asserts,
            device_type: value.device_type.into(),
        }
    }
}

impl From<helium_entity_manager::UpdateIotInfoArgsV0> for HotspotInfoUpdate {
    fn from(value: helium_entity_manager::UpdateIotInfoArgsV0) -> Self {
        Self::Iot {
            gain: value.gain.map(|gain| Decimal::new(gain.into(), 1)),
            elevation: value.elevation,
            location: HotspotLocation::from_maybe(value.location),
        }
    }
}

impl From<helium_entity_manager::OnboardIotHotspotArgsV0> for HotspotInfoUpdate {
    fn from(value: helium_entity_manager::OnboardIotHotspotArgsV0) -> Self {
        Self::Iot {
            gain: value.gain.map(|gain| Decimal::new(gain.into(), 1)),
            elevation: value.elevation,
            location: HotspotLocation::from_maybe(value.location),
        }
    }
}

impl From<helium_entity_manager::OnboardDataOnlyIotHotspotArgsV0> for HotspotInfoUpdate {
    fn from(value: helium_entity_manager::OnboardDataOnlyIotHotspotArgsV0) -> Self {
        Self::Iot {
            gain: value.gain.map(|gain| Decimal::new(gain.into(), 1)),
            elevation: value.elevation,
            location: HotspotLocation::from_maybe(value.location),
        }
    }
}

impl From<helium_entity_manager::UpdateMobileInfoArgsV0> for HotspotInfoUpdate {
    fn from(value: helium_entity_manager::UpdateMobileInfoArgsV0) -> Self {
        Self::Mobile {
            location: HotspotLocation::from_maybe(value.location),
        }
    }
}

impl From<helium_entity_manager::OnboardMobileHotspotArgsV0> for HotspotInfoUpdate {
    fn from(value: helium_entity_manager::OnboardMobileHotspotArgsV0) -> Self {
        Self::Mobile {
            location: HotspotLocation::from_maybe(value.location),
        }
    }
}
