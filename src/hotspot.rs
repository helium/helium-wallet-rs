use crate::{
    dao::SubDao,
    keypair::{serde_opt_pubkey, Pubkey},
    result::Result,
};
use angry_purple_tiger::AnimalName;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize, Clone, Copy, clap::ValueEnum, PartialEq, Eq, Default)]
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
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&str)
    }
}

#[derive(Debug, Serialize)]
pub struct Hotspot {
    pub key: helium_crypto::PublicKey,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", with = "serde_opt_pubkey")]
    pub owner: Option<Pubkey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<HashMap<SubDao, HotspotInfo>>,
}

impl Hotspot {
    pub fn for_address(
        key: helium_crypto::PublicKey,
        owner: Option<Pubkey>,
        info: Option<HashMap<SubDao, HotspotInfo>>,
    ) -> Result<Self> {
        let name = key
            .to_string()
            .parse::<AnimalName>()
            // can unwrap safely
            .unwrap()
            .to_string();
        Ok(Self {
            key,
            name,
            owner,
            info,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase", untagged)]
pub enum HotspotInfo {
    Iot {
        asset: String,
        mode: HotspotMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        gain: Option<rust_decimal::Decimal>,
        #[serde(skip_serializing_if = "Option::is_none")]
        elevation: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<String>,
        location_asserts: u16,
    },
    Mobile {
        asset: String,
        mode: HotspotMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<String>,
        location_asserts: u16,
        device_type: MobileDeviceType,
    },
}

#[derive(Debug, Serialize, Clone, Copy, clap::ValueEnum, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MobileDeviceType {
    #[default]
    Cbrs,
    WifiIndoor,
    WifiOutdoor,
}

impl std::fmt::Display for MobileDeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&str)
    }
}

impl From<helium_entity_manager::MobileDeviceTypeV0> for MobileDeviceType {
    fn from(value: helium_entity_manager::MobileDeviceTypeV0) -> Self {
        match value {
            helium_entity_manager::MobileDeviceTypeV0::Cbrs => Self::Cbrs,
            helium_entity_manager::MobileDeviceTypeV0::WifiIndoor => Self::WifiIndoor,
            helium_entity_manager::MobileDeviceTypeV0::WifiOutdoor => Self::WifiOutdoor,
        }
    }
}

impl From<helium_entity_manager::IotHotspotInfoV0> for HotspotInfo {
    fn from(value: helium_entity_manager::IotHotspotInfoV0) -> Self {
        Self::Iot {
            asset: value.asset.to_string(),
            mode: value.is_full_hotspot.into(),
            gain: value
                .gain
                .map(|gain| rust_decimal::Decimal::new(gain.into(), 1)),
            elevation: value.elevation,
            location: value
                .location
                .and_then(|index| h3o::CellIndex::try_from(index).ok().map(|v| v.to_string())),
            location_asserts: value.num_location_asserts,
        }
    }
}

impl From<helium_entity_manager::MobileHotspotInfoV0> for HotspotInfo {
    fn from(value: helium_entity_manager::MobileHotspotInfoV0) -> Self {
        Self::Mobile {
            asset: value.asset.to_string(),
            mode: value.is_full_hotspot.into(),
            location: value
                .location
                .and_then(|index| h3o::CellIndex::try_from(index).ok().map(|v| v.to_string())),
            location_asserts: value.num_location_asserts,
            device_type: value.device_type.into(),
        }
    }
}
