use crate::{
    asset,
    dao::{Dao, SubDao},
    keypair::{serde_pubkey, Keypair, Pubkey, PublicKey},
    result::{anyhow, Context, Error, Result},
    settings::Settings,
    solana_sdk,
};
use anchor_client::{self, solana_sdk::signer::Signer};
use angry_purple_tiger::AnimalName;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, ops::Deref, result::Result as StdResult, str::FromStr};

lazy_static::lazy_static! {
    static ref HOTSPOT_CREATOR: Pubkey = Pubkey::from_str("Fv5hf1Fg58htfC7YEXKNEfkpuogUUQDDTLgjGWxxv48H").unwrap();
}

/// Entity keys are (regrettably) encoded through the bytes of a the b58
/// string form of the helium public key
pub fn key_to_entity(hotspot_key: &helium_crypto::PublicKey) -> Result<Vec<u8>> {
    Ok(bs58::decode(hotspot_key.to_string()).into_vec()?)
}

pub fn key_to_asset<C: Clone + Deref<Target = impl Signer>>(
    client: &anchor_client::Client<C>,
    hotspot_key: &helium_crypto::PublicKey,
) -> Result<helium_entity_manager::KeyToAssetV0> {
    let entity_key = key_to_entity(hotspot_key)?;
    asset::account_for_entity_key(client, &entity_key)
}

pub fn get_asset(
    settings: &Settings,
    hotspot_key: &helium_crypto::PublicKey,
) -> Result<asset::Asset> {
    let client = settings.mk_anchor_client(Keypair::void())?;
    let asset_account = key_to_asset(&client, hotspot_key)?;
    asset::get(settings, &asset_account)
}

pub fn get_asset_proof(
    settings: &Settings,
    hotspot_key: &helium_crypto::PublicKey,
) -> Result<asset::AsssetProof> {
    let client = settings.mk_anchor_client(Keypair::void())?;
    let asset_account = key_to_asset(&client, hotspot_key)?;
    asset::get_proof(settings, &asset_account)
}

pub fn get_for_owner(settings: &Settings, owner: &Pubkey) -> Result<Vec<Hotspot>> {
    let assets = asset::get_assets(settings, &HOTSPOT_CREATOR, owner)?;
    assets
        .into_iter()
        .map(Hotspot::try_from)
        .collect::<Result<Vec<Hotspot>>>()
}

pub fn get_info_in_subdao(
    settings: &Settings,
    subdao: &SubDao,
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
        SubDao::Iot => {
            maybe_info(program.account::<helium_entity_manager::IotHotspotInfoV0>(hotspot_key))
        }
        SubDao::Mobile => {
            maybe_info(program.account::<helium_entity_manager::MobileHotspotInfoV0>(hotspot_key))
        }
    }
}

pub fn get_info(
    settings: &Settings,
    subdaos: &[SubDao],
    key: &helium_crypto::PublicKey,
) -> Result<HashMap<SubDao, HotspotInfo>> {
    let infos = subdaos
        .par_iter()
        .filter_map(|subdao| match get_info_in_subdao(settings, subdao, key) {
            Ok(Some(metadata)) => Some(Ok((*subdao, metadata))),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        })
        .collect::<Result<Vec<(SubDao, HotspotInfo)>>>()?;
    Ok(HashMap::from_iter(infos))
}

pub fn assert<C: Clone + Deref<Target = impl Signer> + PublicKey>(
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
        return Err(anyhow!(
            "Onboard transaction request failed: {} {}",
            onboarding_resp.code,
            onboarding_resp
                .error_message
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }

    let mut tx = onboarding_resp
        .data
        .ok_or_else(|| anyhow!("No transaction data returned"))
        .and_then(|resp_data| {
            bincode::deserialize::<solana_sdk::transaction::Transaction>(
                &resp_data.solana_transactions[0].data,
            )
            .map_err(Error::from)
        })?;

    tx.try_partial_sign(&[&*keypair], tx.message.recent_blockhash)?;
    Ok(tx)
}

pub mod dataonly {
    use super::*;
    use crate::token::Token;
    use helium_proto::{BlockchainTxnAddGatewayV1, Message};

    pub fn onboard<C: Clone + Deref<Target = impl Signer> + PublicKey>(
        settings: &Settings,
        hotspot_key: &helium_crypto::PublicKey,
        assertion: HotspotAssertion,
        keypair: C,
    ) -> Result<solana_sdk::transaction::Transaction> {
        use helium_entity_manager::accounts::OnboardDataOnlyIotHotspotV0;
        fn mk_dataonly_onboard<C: Clone + Deref<Target = impl Signer>>(
            program: &anchor_client::Program<C>,
            hotspot_key: &helium_crypto::PublicKey,
        ) -> Result<OnboardDataOnlyIotHotspotV0> {
            let dao = Dao::Hnt;

            let entity_key = key_to_entity(hotspot_key)?;
            let data_only_config_key = dao.dataonly_config_key();
            let data_only_config_acc = program
                .account::<helium_entity_manager::DataOnlyConfigV0>(data_only_config_key)
                .context(format!(
                    "while getting data only config, {data_only_config_key}"
                ))?;

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
                key_to_asset: dao.key_to_asset(&entity_key),
                sub_dao: SubDao::Iot.key(),
                dc_mint: *Token::Dc.mint(),
                dc: SubDao::dc_key(),
                compression_program: account_compression_cpi::id(),
                data_credits_program: data_credits::id(),
                helium_sub_daos_program: helium_sub_daos::id(),
                token_program: anchor_spl::token::ID,
                associated_token_program: spl_associated_token_account::id(),
                system_program: solana_sdk::system_program::id(),
            })
        }

        let client = settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(helium_entity_manager::id())?;

        let asset = get_asset(settings, hotspot_key)?;
        let asset_proof = get_asset_proof(settings, hotspot_key)?;

        let onboard_accounts = mk_dataonly_onboard(&program, hotspot_key)
            .context("constructing nboarding accounts")?;
        let mut ixs = program
            .request()
            .args(
                helium_entity_manager::instruction::OnboardDataOnlyIotHotspotV0 {
                    args: helium_entity_manager::OnboardDataOnlyIotHotspotArgsV0 {
                        data_hash: asset.compression.data_hash,
                        creator_hash: asset.compression.creator_hash,
                        index: asset.compression.leaf_id.try_into()?,
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

    pub fn issue<C: Clone + Deref<Target = impl Signer> + PublicKey>(
        settings: &Settings,
        verifier: &str,
        add_tx: &mut BlockchainTxnAddGatewayV1,
        keypair: C,
    ) -> Result<solana_sdk::transaction::Transaction> {
        use helium_entity_manager::accounts::IssueDataOnlyEntityV0;
        fn mk_dataonly_issue<C: Clone + Deref<Target = impl Signer>>(
            program: &anchor_client::Program<C>,
            entity_key: &[u8],
        ) -> Result<IssueDataOnlyEntityV0> {
            use anchor_client::anchor_lang::Id;
            let dao = Dao::Hnt;
            let dataonly_config_key = dao.dataonly_config_key();
            let dataonly_config_acc = program
                .account::<helium_entity_manager::DataOnlyConfigV0>(dataonly_config_key)
                .context(format!(
                    "while getting data only config, {dataonly_config_key}"
                ))?;

            Ok(IssueDataOnlyEntityV0 {
                payer: program.payer(),
                ecc_verifier: Pubkey::from_str(
                    helium_entity_manager::instructions::issue_entity_v0::ECC_VERIFIER,
                )?,
                collection: dataonly_config_acc.collection,
                collection_metadata: dao.collection_metadata_key(&dataonly_config_acc.collection),
                collection_master_edition: dao
                    .collection_master_edition_key(&dataonly_config_acc.collection),
                data_only_config: dataonly_config_key,
                entity_creator: dao.entity_creator_key(),
                dao: dao.key(),
                key_to_asset: dao.key_to_asset(entity_key),
                tree_authority: dao.merkle_tree_authority(&dataonly_config_acc.merkle_tree),
                recipient: program.payer(),
                merkle_tree: dataonly_config_acc.merkle_tree,
                data_only_escrow: dao.dataonly_escrow_key(),
                bubblegum_signer: dao.bubblegum_signer(),
                token_metadata_program: lazy_distributor::token_metadata::ID,
                log_wrapper: account_compression_cpi::Noop::id(),
                bubblegum_program: bubblegum_cpi::id(),
                compression_program: account_compression_cpi::id(),
                system_program: solana_sdk::system_program::id(),
            })
        }

        let client = settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(helium_entity_manager::id())?;
        let hotspot_key = helium_crypto::PublicKey::from_bytes(&add_tx.gateway)?;
        let entity_key = key_to_entity(&hotspot_key)?;

        let issue_entity_accounts = mk_dataonly_issue(&program, &entity_key)?;
        let compute_ix =
            solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(500000);

        let ix = program
            .request()
            .args(helium_entity_manager::instruction::IssueDataOnlyEntityV0 {
                args: helium_entity_manager::IssueDataOnlyEntityArgsV0 { entity_key },
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
        let serialized_tx = hex::encode(bincode::serialize(&tx)?);
        let response = client
            .post(format!("{}/verify", verifier))
            .json(&VerifyRequest {
                transaction: &serialized_tx,
                msg: &hex::encode(msg),
                signature: &hex::encode(signature),
            })
            .send()?
            .json::<VerifyResponse>()
            .context("While verifying add gateway txn signature")?;
        let signed_tx = bincode::deserialize(&hex::decode(response.transaction)?)?;
        Ok(signed_tx)
    }
}

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

impl TryFrom<asset::Asset> for Hotspot {
    type Error = Error;
    fn try_from(value: asset::Asset) -> Result<Self> {
        value
            .content
            .metadata
            .get_attribute("ecc_compact")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("no entity key found"))
            .and_then(|str| helium_crypto::PublicKey::from_str(str).map_err(Error::from))
            .map(|hotspot_key| Self::with_hotspot_key(hotspot_key, value.ownership.owner))
    }
}

pub struct HotspotAssertion {
    pub location: Option<h3o::CellIndex>,
    pub gain: Option<i32>,
    pub elevation: Option<i32>,
}

impl TryFrom<(Option<f64>, Option<f64>, Option<i32>, Option<f64>)> for HotspotAssertion {
    type Error = Error;
    fn try_from(
        value: (Option<f64>, Option<f64>, Option<i32>, Option<f64>),
    ) -> StdResult<Self, Self::Error> {
        let (lat, lon, elevation, gain) = value;
        let location: Option<h3o::CellIndex> = match (lat, lon) {
            (Some(lat), Some(lon)) => {
                Some(h3o::LatLng::new(lat, lon)?.to_cell(h3o::Resolution::Twelve))
            }
            (None, None) => None,
            _ => anyhow::bail!("Both lat and lon must be specified"),
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
