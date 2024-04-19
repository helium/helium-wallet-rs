use crate::{
    asset,
    dao::{Dao, SubDao},
    is_zero,
    keypair::{pubkey, serde_pubkey, Keypair, Pubkey, PublicKey},
    result::{DecodeError, EncodeError, Error, Result},
    settings::{DasClient, DasSearchAssetsParams, Settings},
};
use anchor_client::{self, solana_sdk::signer::Signer};
use angry_purple_tiger::AnimalName;
use futures::{
    stream::{self, StreamExt, TryStreamExt},
    TryFutureExt,
};
use helium_anchor_gen::{data_credits, helium_entity_manager, helium_sub_daos};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, ops::Deref, result::Result as StdResult, str::FromStr};

pub const HOTSPOT_CREATOR: Pubkey = pubkey!("Fv5hf1Fg58htfC7YEXKNEfkpuogUUQDDTLgjGWxxv48H");
pub const ECC_VERIFIER: Pubkey = pubkey!("eccCd1PHAPSTNLUtDzihhPmFPTqGPQn7kgLyjf6dYTS");

pub async fn for_owner(settings: &Settings, owner: &Pubkey) -> Result<Vec<Hotspot>> {
    let assets = asset::for_owner(settings, &HOTSPOT_CREATOR, owner).await?;
    assets
        .into_iter()
        .map(Hotspot::try_from)
        .collect::<Result<Vec<Hotspot>>>()
}

pub async fn search(client: &DasClient, params: &DasSearchAssetsParams) -> Result<HotspotPage> {
    let asset_page = asset::search(client, params).await?;
    HotspotPage::try_from(asset_page)
}

pub async fn get(settings: &Settings, hotspot_key: &helium_crypto::PublicKey) -> Result<Hotspot> {
    let client = settings.mk_anchor_client(Keypair::void())?;
    let asset_account = asset::account_for_entity_key(&client, hotspot_key).await?;
    let asset = asset::get(settings, &asset_account).await?;
    asset.try_into()
}

pub async fn get_with_info(
    settings: &Settings,
    subdaos: &[SubDao],
    hotspot_key: &helium_crypto::PublicKey,
) -> Result<Hotspot> {
    let (mut hotspot, info) = futures::try_join!(
        get(settings, hotspot_key),
        info::get(settings, subdaos, hotspot_key)
    )?;
    if !info.is_empty() {
        hotspot.info = Some(info);
    }
    Ok(hotspot)
}

pub mod info {
    use super::*;

    pub async fn for_subdao(
        settings: &Settings,
        subdao: SubDao,
        key: &helium_crypto::PublicKey,
    ) -> Result<Option<HotspotInfo>> {
        fn maybe_info<T>(
            result: StdResult<T, anchor_client::ClientError>,
        ) -> Result<Option<HotspotInfo>>
        where
            T: Into<HotspotInfo>,
        {
            match result {
                Ok(account) => Ok(Some(account.into())),
                Err(anchor_client::ClientError::AccountNotFound) => Ok(None),
                Err(err) => Err(err.into()),
            }
        }

        let client = settings.mk_anchor_client(Keypair::void())?;
        let hotspot_key = subdao.info_key_for_helium_key(key)?;
        let program = client.program(helium_entity_manager::id())?;
        match subdao {
            SubDao::Iot => maybe_info(
                program
                    .account::<helium_entity_manager::IotHotspotInfoV0>(hotspot_key)
                    .await,
            ),
            SubDao::Mobile => maybe_info(
                program
                    .account::<helium_entity_manager::MobileHotspotInfoV0>(hotspot_key)
                    .await,
            ),
        }
    }

    pub async fn get(
        settings: &Settings,
        subdaos: &[SubDao],
        key: &helium_crypto::PublicKey,
    ) -> Result<HashMap<SubDao, HotspotInfo>> {
        stream::iter(subdaos.to_vec())
            .map(|subdao| {
                for_subdao(settings, subdao, key)
                    .map_ok(move |maybe_metadata| maybe_metadata.map(|metadata| (subdao, metadata)))
            })
            .buffer_unordered(10)
            .filter_map(|result| async move { result.transpose() })
            .try_collect::<Vec<(SubDao, HotspotInfo)>>()
            .map_ok(HashMap::from_iter)
            .await
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OnboardingError {
    #[error("onboarding txn request: {code} {reason}")]
    Error { code: u32, reason: String },
    #[error("no transaction data in response")]
    NoTxnData,
    #[error("invalid transaction data in resopnse")]
    InvalidTxnData,
}

impl From<OnboardingResponse> for OnboardingError {
    fn from(value: OnboardingResponse) -> Self {
        Self::Error {
            code: value.code,
            reason: value.error_message.unwrap_or("unknown".to_string()),
        }
    }
}

pub async fn assert<C: Clone + Deref<Target = impl Signer> + PublicKey>(
    onboarding_server: &str,
    subdao: SubDao,
    hotspot: &helium_crypto::PublicKey,
    assertion: HotspotAssertion,
    keypair: C,
) -> Result<solana_sdk::transaction::Transaction> {
    let client = Settings::mk_rest_client()?;
    let url = format!(
        "{}/transactions/{}/update-metadata",
        onboarding_server, subdao
    );
    let params = json!({
        "entityKey": hotspot.to_string(),
        "wallet": keypair.public_key().to_string(),
        "location": serde_json::Value::from(
            assertion
            .location
            .map(|location| u64::from(location).to_string())),
        "gain": serde_json::Value::from(assertion.gain),
        "elevation": serde_json::Value::from(assertion.elevation),
    });

    let resp = client.post(url).json(&params).send()?.error_for_status()?;
    let onboarding_resp = resp.json::<OnboardingResponse>()?;
    if !onboarding_resp.success {
        return Err(OnboardingError::from(onboarding_resp).into());
    }

    let mut tx = onboarding_resp
        .data
        .ok_or(OnboardingError::NoTxnData)
        .and_then(|resp_data| {
            bincode::deserialize::<solana_sdk::transaction::Transaction>(
                &resp_data.solana_transactions[0].data,
            )
            .map_err(|_| OnboardingError::InvalidTxnData)
        })?;

    tx.try_partial_sign(&[&*keypair], tx.message.recent_blockhash)?;
    Ok(tx)
}

pub mod dataonly {
    use super::*;
    use crate::{
        entity_key::AsEntityKey,
        programs::{
            MPL_BUBBLEGUM_PROGRAM_ID, SPL_ACCOUNT_COMPRESSION_PROGRAM_ID, SPL_NOOP_PROGRAM_ID,
            TOKEN_METADATA_PROGRAM_ID,
        },
        token::Token,
    };
    use helium_proto::{BlockchainTxnAddGatewayV1, Message};

    pub async fn onboard<C: Clone + Deref<Target = impl Signer> + PublicKey>(
        settings: &Settings,
        hotspot_key: &helium_crypto::PublicKey,
        assertion: HotspotAssertion,
        keypair: C,
    ) -> Result<solana_sdk::transaction::Transaction> {
        use helium_entity_manager::accounts::OnboardDataOnlyIotHotspotV0;
        async fn mk_dataonly_onboard<C: Clone + Deref<Target = impl Signer>>(
            program: &anchor_client::Program<C>,
            hotspot_key: &helium_crypto::PublicKey,
        ) -> Result<OnboardDataOnlyIotHotspotV0> {
            let dao = Dao::Hnt;

            let entity_key = hotspot_key.as_entity_key();
            let data_only_config_key = dao.dataonly_config_key();
            let data_only_config_acc = program
                .account::<helium_entity_manager::DataOnlyConfigV0>(data_only_config_key)
                .await?;

            Ok(OnboardDataOnlyIotHotspotV0 {
                payer: program.payer(),
                dc_fee_payer: program.payer(),
                iot_info: SubDao::Iot.info_key(&entity_key),
                hotspot_owner: program.payer(),
                merkle_tree: data_only_config_acc.merkle_tree,
                dc_burner: Token::Dc.associated_token_adress(&program.payer()),
                rewardable_entity_config: SubDao::Iot.rewardable_entity_config_key(),
                data_only_config: data_only_config_key,
                dao: dao.key(),
                key_to_asset: dao.key_to_asset_key(&entity_key),
                sub_dao: SubDao::Iot.key(),
                dc_mint: *Token::Dc.mint(),
                dc: SubDao::dc_key(),
                compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                data_credits_program: data_credits::id(),
                helium_sub_daos_program: helium_sub_daos::id(),
                token_program: anchor_spl::token::ID,
                associated_token_program: spl_associated_token_account::id(),
                system_program: solana_sdk::system_program::id(),
            })
        }

        let client = settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(helium_entity_manager::id())?;

        let asset_account = asset::account_for_entity_key(&client, hotspot_key).await?;
        let asset = asset::get(settings, &asset_account).await?;
        let asset_proof = asset::proof::get(settings, &asset_account).await?;

        let onboard_accounts = mk_dataonly_onboard(&program, hotspot_key).await?;
        let mut ixs = program
            .request()
            .args(
                helium_entity_manager::instruction::OnboardDataOnlyIotHotspotV0 {
                    _args: helium_entity_manager::OnboardDataOnlyIotHotspotArgsV0 {
                        data_hash: asset.compression.data_hash,
                        creator_hash: asset.compression.creator_hash,
                        index: asset
                            .compression
                            .leaf_id
                            .try_into()
                            .map_err(DecodeError::from)?,
                        root: asset_proof.root.to_bytes(),
                        elevation: assertion.elevation,
                        gain: assertion.gain,
                        location: assertion.location.map(Into::into),
                    },
                },
            )
            .accounts(onboard_accounts)
            .instructions()?;
        ixs[0]
            .accounts
            .extend_from_slice(&asset_proof.proof()?[0..3]);

        let mut tx =
            solana_sdk::transaction::Transaction::new_with_payer(&ixs, Some(&keypair.public_key()));
        let blockhash = program.rpc().get_latest_blockhash()?;

        tx.try_sign(&[&*keypair], blockhash)?;

        Ok(tx)
    }

    pub async fn issue<C: Clone + Deref<Target = impl Signer> + PublicKey>(
        settings: &Settings,
        verifier: &str,
        add_tx: &mut BlockchainTxnAddGatewayV1,
        keypair: C,
    ) -> Result<solana_sdk::transaction::Transaction> {
        use helium_entity_manager::accounts::IssueDataOnlyEntityV0;
        async fn mk_dataonly_issue<C: Clone + Deref<Target = impl Signer>>(
            program: &anchor_client::Program<C>,
            entity_key: &[u8],
        ) -> Result<IssueDataOnlyEntityV0> {
            let dao = Dao::Hnt;
            let dataonly_config_key = dao.dataonly_config_key();
            let dataonly_config_acc = program
                .account::<helium_entity_manager::DataOnlyConfigV0>(dataonly_config_key)
                .await?;

            Ok(IssueDataOnlyEntityV0 {
                payer: program.payer(),
                ecc_verifier: ECC_VERIFIER,
                collection: dataonly_config_acc.collection,
                collection_metadata: dao.collection_metadata_key(&dataonly_config_acc.collection),
                collection_master_edition: dao
                    .collection_master_edition_key(&dataonly_config_acc.collection),
                data_only_config: dataonly_config_key,
                entity_creator: dao.entity_creator_key(),
                dao: dao.key(),
                key_to_asset: dao.key_to_asset_key(&entity_key),
                tree_authority: dao.merkle_tree_authority(&dataonly_config_acc.merkle_tree),
                recipient: program.payer(),
                merkle_tree: dataonly_config_acc.merkle_tree,
                data_only_escrow: dao.dataonly_escrow_key(),
                bubblegum_signer: dao.bubblegum_signer(),
                token_metadata_program: TOKEN_METADATA_PROGRAM_ID,
                log_wrapper: SPL_NOOP_PROGRAM_ID,
                bubblegum_program: MPL_BUBBLEGUM_PROGRAM_ID,
                compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                system_program: solana_sdk::system_program::id(),
            })
        }

        let client = settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(helium_entity_manager::id())?;
        let hotspot_key = helium_crypto::PublicKey::from_bytes(&add_tx.gateway)?;
        let entity_key = hotspot_key.as_entity_key();

        let issue_entity_accounts = mk_dataonly_issue(&program, &entity_key).await?;
        let compute_ix =
            solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(500000);

        let ix = program
            .request()
            .args(helium_entity_manager::instruction::IssueDataOnlyEntityV0 {
                _args: helium_entity_manager::IssueDataOnlyEntityArgsV0 { entity_key },
            })
            .accounts(issue_entity_accounts)
            .instruction(compute_ix)
            .instructions()?;

        let mut tx =
            solana_sdk::transaction::Transaction::new_with_payer(&ix, Some(&keypair.public_key()));
        let blockhash = program.rpc().get_latest_blockhash()?;

        tx.try_partial_sign(&[&*keypair], blockhash)?;

        let sig = add_tx.gateway_signature.clone();
        add_tx.gateway_signature = vec![];
        let msg = add_tx.encode_to_vec();

        let signed_tx = verify_helium_key(verifier, &msg, &sig, tx)?;
        Ok(signed_tx)
    }

    fn verify_helium_key(
        verifier: &str,
        msg: &[u8],
        signature: &[u8],
        tx: solana_sdk::transaction::Transaction,
    ) -> Result<solana_sdk::transaction::Transaction> {
        #[derive(Deserialize, Serialize, Default)]
        struct VerifyRequest<'a> {
            // hex encoded solana transaction
            pub transaction: &'a str,
            // hex encoded signed message
            pub msg: &'a str,
            // hex encoded signature
            pub signature: &'a str,
        }
        #[derive(Deserialize, Serialize, Default)]
        struct VerifyResponse {
            // hex encoded solana transaction
            pub transaction: String,
        }

        let client = Settings::mk_rest_client()?;
        let serialized_tx = hex::encode(bincode::serialize(&tx).map_err(EncodeError::from)?);
        let response = client
            .post(format!("{}/verify", verifier))
            .json(&VerifyRequest {
                transaction: &serialized_tx,
                msg: &hex::encode(msg),
                signature: &hex::encode(signature),
            })
            .send()?
            .json::<VerifyResponse>()?;
        let signed_tx =
            bincode::deserialize(&hex::decode(response.transaction).map_err(DecodeError::from)?)
                .map_err(DecodeError::from)?;
        Ok(signed_tx)
    }
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Default)]
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
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&str)
    }
}

#[derive(Serialize, Clone)]
pub struct HotspotPage {
    pub total: u32,
    pub limit: u32,
    pub page: u32,
    pub items: Vec<Hotspot>,
}

impl TryFrom<asset::AssetPage> for HotspotPage {
    type Error = Error;
    fn try_from(value: asset::AssetPage) -> StdResult<Self, Self::Error> {
        Ok(Self {
            total: value.total,
            limit: value.limit,
            page: value.page,
            items: value
                .items
                .into_iter()
                .map(Hotspot::try_from)
                .collect::<Result<Vec<Hotspot>>>()?,
        })
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Hotspot {
    pub key: helium_crypto::PublicKey,
    pub name: String,
    #[serde(with = "serde_pubkey")]
    pub owner: Pubkey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<HashMap<SubDao, HotspotInfo>>,
}

impl Hotspot {
    pub fn with_hotspot_key(key: helium_crypto::PublicKey, owner: Pubkey) -> Self {
        let name = key
            .to_string()
            .parse::<AnimalName>()
            // can unwrap safely
            .unwrap()
            .to_string();
        Self {
            key,
            name,
            owner,
            info: None,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase", untagged)]
pub enum HotspotInfo {
    Iot {
        #[serde(skip_serializing_if = "Option::is_none")]
        asset: Option<String>,
        mode: HotspotMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        gain: Option<rust_decimal::Decimal>,
        #[serde(skip_serializing_if = "Option::is_none")]
        elevation: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<String>,
        #[serde(skip_serializing_if = "is_zero")]
        location_asserts: u16,
    },
    Mobile {
        #[serde(skip_serializing_if = "Option::is_none")]
        asset: Option<String>,
        mode: HotspotMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<String>,
        #[serde(skip_serializing_if = "is_zero")]
        location_asserts: u16,
        device_type: MobileDeviceType,
    },
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
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
            asset: Some(value.asset.to_string()),
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
            asset: Some(value.asset.to_string()),
            mode: value.is_full_hotspot.into(),
            location: value
                .location
                .and_then(|index| h3o::CellIndex::try_from(index).ok().map(|v| v.to_string())),
            location_asserts: value.num_location_asserts,
            device_type: value.device_type.into(),
        }
    }
}

impl TryFrom<asset::Asset> for Hotspot {
    type Error = Error;
    fn try_from(value: asset::Asset) -> Result<Self> {
        value
            .content
            .metadata
            .get_attribute("ecc_compact")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DecodeError::other("no entity key found"))
            .and_then(|str| helium_crypto::PublicKey::from_str(str).map_err(DecodeError::from))
            .map(|hotspot_key| Self::with_hotspot_key(hotspot_key, value.ownership.owner))
            .map_err(Error::from)
    }
}

pub struct HotspotAssertion {
    pub location: Option<h3o::CellIndex>,
    pub gain: Option<i32>,
    pub elevation: Option<i32>,
}

impl TryFrom<(Option<f64>, Option<f64>, Option<i32>, Option<f64>)> for HotspotAssertion {
    type Error = EncodeError;
    fn try_from(
        value: (Option<f64>, Option<f64>, Option<i32>, Option<f64>),
    ) -> StdResult<Self, Self::Error> {
        let (lat, lon, elevation, gain) = value;
        let location: Option<h3o::CellIndex> = match (lat, lon) {
            (Some(lat), Some(lon)) => Some(
                h3o::LatLng::new(lat, lon)
                    .map_err(EncodeError::from)?
                    .to_cell(h3o::Resolution::Twelve),
            ),
            (None, None) => None,
            _ => return Err(EncodeError::other("Both lat and lon must be specified")),
        };

        Ok(Self {
            elevation,
            location,
            gain: gain.map(|g| (g * 10.0).trunc() as i32),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingResponse {
    code: u32,
    success: bool,
    error_message: Option<String>,
    data: Option<OnboardingResponseData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingResponseData {
    solana_transactions: Vec<OnboardingResponseSolanaTransaction>,
}

#[derive(Deserialize)]
struct OnboardingResponseSolanaTransaction {
    data: Vec<u8>,
}
