use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    anchor_spl, asset, bs58,
    client::{DasClient, DasSearchAssetsParams, GetAnchorAccount, SolanaRpcClient},
    dao::{Dao, SubDao},
    data_credits,
    error::{DecodeError, EncodeError, Error},
    helium_entity_manager, is_zero,
    keypair::{pubkey, serde_pubkey, Keypair, Pubkey},
    kta, mk_transaction_with_blockhash, onboarding, priority_fee,
    programs::SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
    solana_sdk::{
        instruction::{AccountMeta, Instruction},
        signer::Signer,
    },
    token::Token,
    TransactionOpts, TransactionWithBlockhash,
};
use angry_purple_tiger::AnimalName;
use chrono::Utc;
use futures::TryFutureExt;
use itertools::{izip, Itertools};
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
    // Get all kta keys for the Hotspots in the assets for the given owner
    let (kta_keys, hotspot_assets): (Vec<Pubkey>, Vec<asset::Asset>) = assets
        .into_iter()
        .filter(|asset| asset.is_symbol("HOTSPOT"))
        .map(|asset| asset.kta_key().map(|kta_key| (kta_key, asset)))
        .collect::<Result<Vec<(Pubkey, asset::Asset)>, Error>>()?
        .into_iter()
        .unzip();
    // Get all ktas in one go
    let ktas = kta::get_many(&kta_keys).await?;
    // Then construct Hotspots from assets and ktas
    izip!(ktas, hotspot_assets)
        .map(|(kta, asset)| Hotspot::from_asset_with_kta(kta, asset))
        .try_collect()
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

pub async fn direct_update_transaction<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot: &helium_crypto::PublicKey,
    update: HotspotInfoUpdate,
    owner: &Pubkey,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
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
                    dc: Dao::dc_key(),
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

    macro_rules! mk_update_data {
        ($ix_struct:ident, $arg_struct:ident, $($manual_fields:tt)*) => {
            $ix_struct {
                _args: $arg_struct {
                    root: asset_proof.root.to_bytes(),
                    data_hash: asset.compression.data_hash,
                    creator_hash: asset.compression.creator_hash,
                    index: asset.compression.leaf_id()?,
                    $($manual_fields)*
                },
            }
            .data()
        };
    }

    let mut accounts = mk_accounts(update.subdao(), &kta, &asset, owner);
    accounts.extend_from_slice(&asset_proof.proof(Some(3))?);

    use helium_entity_manager::{
        instruction::{
            UpdateIotInfoV0 as IxUpdateIotInfo, UpdateMobileInfoV0 as IxUpdateMobileInfo,
        },
        UpdateIotInfoArgsV0 as ArgsUpdateIotInfo, UpdateMobileInfoArgsV0 as ArgsUpdateMobileInfo,
    };
    let data = match update.subdao() {
        SubDao::Iot => {
            mk_update_data!(IxUpdateIotInfo , ArgsUpdateIotInfo,
                elevation: *update.elevation(),
                gain: update.gain_i32(),
                location: update.location_u64())
        }
        SubDao::Mobile => {
            mk_update_data!(IxUpdateMobileInfo, ArgsUpdateMobileInfo,
            location: update.location_u64(),
            deployment_info: None,
            )
        }
    };
    let ix = Instruction {
        program_id: helium_entity_manager::id(),
        accounts: accounts.to_account_metas(None),
        data,
    };

    let ixs = &[
        priority_fee::compute_budget_instruction(200_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &accounts,
            opts.min_priority_fee,
        )
        .await?,
        ix,
    ];

    mk_transaction_with_blockhash(client, ixs, owner).await
}

pub async fn direct_update<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot: &helium_crypto::PublicKey,
    update: HotspotInfoUpdate,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let mut txn =
        direct_update_transaction(client, hotspot, update, &keypair.pubkey(), opts).await?;
    txn.try_sign(&[keypair])?;
    Ok(txn)
}

pub async fn update<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    onboarding_server: Option<String>,
    hotspot: &helium_crypto::PublicKey,
    update: HotspotInfoUpdate,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let public_key = keypair.pubkey();
    if let Some(server) = onboarding_server {
        let onboarding_client = onboarding::Client::new(&server);
        let mut tx = onboarding_client
            .get_update_txn(hotspot, &public_key, update)
            .await?;
        tx.try_partial_sign(&[keypair], tx.message.recent_blockhash)?;
        todo!("thread through helium-lib Transaction")
        // return Ok(tx);
    };
    let tx = direct_update(client, hotspot, update, keypair, opts).await?;
    Ok(tx)
}

/// Get an unsigned transaction for a Hotspot transfer.
///
/// The Hotspot is transferred from the owner of the Hotspot to the given recipient
/// Note that the owner is currently expected to sign this transaction and pay for
/// transaction fees.
pub async fn transfer_transaction<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot_key: &helium_crypto::PublicKey,
    recipient: &Pubkey,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let kta = kta::for_entity_key(hotspot_key).await?;
    asset::transfer_transaction(client, &kta.asset, recipient, opts).await
}

pub async fn transfer<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot_key: &helium_crypto::PublicKey,
    recipient: &Pubkey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let kta = kta::for_entity_key(hotspot_key).await?;
    asset::transfer(client, &kta.asset, recipient, keypair, opts).await
}

pub async fn burn_transaction<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot_key: &helium_crypto::PublicKey,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let kta = kta::for_entity_key(hotspot_key).await?;
    asset::burn_transaction(client, &kta.asset, opts).await
}

pub async fn burn<C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
    client: &C,
    hotspot_key: &helium_crypto::PublicKey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let kta = kta::for_entity_key(hotspot_key).await?;
    asset::burn(client, &kta.asset, keypair, opts).await
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
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub burnt: bool,
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
            burnt: asset.burnt,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        deployment_info: Option<MobileDeploymentInfo>,
    },
}

#[derive(Debug, Serialize, Clone, Hash)]
#[serde(rename_all = "lowercase", untagged)]
pub enum MobileDeploymentInfo {
    WifiInfo {
        #[serde(skip_serializing_if = "is_zero")]
        antenna: u32,
        // the height of the Hotspot above ground level in whole meters
        #[serde(skip_serializing_if = "is_zero")]
        elevation: i32,
        #[serde(skip_serializing_if = "is_zero")]
        azimuth: Decimal,
        #[serde(skip_serializing_if = "is_zero")]
        mechanical_down_tilt: Decimal,
        #[serde(skip_serializing_if = "is_zero")]
        electrical_down_tilt: Decimal,
    },
    CbrsInfo {
        radio_infos: Vec<CbrsRadioInfo>,
    },
}

#[derive(Debug, Serialize, Clone, Hash)]
#[serde(rename_all = "lowercase")]
pub struct CbrsRadioInfo {
    // CBSD_ID or radio
    pub radio_id: String,
    // The asserted elevation of the gateway above ground level in whole meters
    #[serde(skip_serializing_if = "is_zero")]
    pub elevation: i32,
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
            deployment_info: value.deployment_info.map(MobileDeploymentInfo::from),
        }
    }
}

impl From<helium_entity_manager::MobileDeploymentInfoV0> for MobileDeploymentInfo {
    fn from(value: helium_entity_manager::MobileDeploymentInfoV0) -> Self {
        match value {
            helium_entity_manager::MobileDeploymentInfoV0::WifiInfoV0 {
                antenna,
                elevation,
                azimuth,
                mechanical_down_tilt,
                electrical_down_tilt,
            } => Self::WifiInfo {
                antenna,
                elevation,
                azimuth: Decimal::new(azimuth as i64, 2),
                mechanical_down_tilt: Decimal::new(mechanical_down_tilt as i64, 2),
                electrical_down_tilt: Decimal::new(electrical_down_tilt as i64, 2),
            },
            helium_entity_manager::MobileDeploymentInfoV0::CbrsInfoV0 { radio_infos } => {
                Self::CbrsInfo {
                    radio_infos: radio_infos.into_iter().map(CbrsRadioInfo::from).collect(),
                }
            }
        }
    }
}

impl From<helium_entity_manager::RadioInfoV0> for CbrsRadioInfo {
    fn from(value: helium_entity_manager::RadioInfoV0) -> Self {
        Self {
            radio_id: value.radio_id,
            elevation: value.elevation,
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

impl From<helium_entity_manager::OnboardDataOnlyMobileHotspotArgsV0> for HotspotInfoUpdate {
    fn from(value: helium_entity_manager::OnboardDataOnlyMobileHotspotArgsV0) -> Self {
        Self::Mobile {
            location: HotspotLocation::from_maybe(value.location),
        }
    }
}
