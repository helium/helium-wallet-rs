use super::{Client, Settings};
use crate::keypair::{serde_pubkey, PublicKey};
use crate::{
    dao::{Dao, SubDao},
    hotspot::{Hotspot, HotspotInfo},
    keypair::{Keypair, Pubkey},
    result::{anyhow, Error, Result},
    token::Token,
};
use anchor_client::solana_sdk::{self, signer::Signer, system_program};
use anyhow::Context;
use helium_proto::{BlockchainTxnAddGatewayV1, Message};
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::json;
use spl_associated_token_account::get_associated_token_address;
use std::{collections::HashMap, ops::Deref, rc::Rc, result::Result as StdResult, str::FromStr};

pub struct HotspotAssertion {
    pub location: Option<u64>,
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
            location: location.map(u64::from),
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

#[derive(Deserialize)]
struct PagedResult {
    items: Vec<HotspotResult>,
}

impl TryFrom<PagedResult> for Vec<Hotspot> {
    type Error = Error;
    fn try_from(value: PagedResult) -> StdResult<Self, Self::Error> {
        value
            .items
            .into_iter()
            .map(Hotspot::try_from)
            .collect::<Result<Vec<Hotspot>>>()
    }
}

#[derive(Debug, Deserialize)]
struct HotspotResult {
    content: HotspotContent,
}

#[derive(Debug, Deserialize)]
struct HotspotContent {
    metadata: HotspotMetadata,
}

#[derive(Debug, Deserialize)]
struct HotspotMetadata {
    attributes: Vec<HotspotMetadataAttribute>,
}

impl HotspotMetadata {
    fn get_attribute(&self, trait_type: &str) -> Option<&serde_json::Value> {
        self.attributes
            .iter()
            .filter(|entry| entry.trait_type == trait_type)
            .collect::<Vec<&HotspotMetadataAttribute>>()
            .first()
            .map(|entry| &entry.value)
    }
}

#[derive(Debug, Deserialize)]
struct HotspotMetadataAttribute {
    value: serde_json::Value,
    trait_type: String,
}

impl TryFrom<HotspotResult> for Hotspot {
    type Error = Error;
    fn try_from(value: HotspotResult) -> StdResult<Self, Self::Error> {
        let ecc_key = value
            .content
            .metadata
            .get_attribute("ecc_compact")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("no ecc_compact key found"))
            .and_then(|str| helium_crypto::PublicKey::from_str(str).map_err(Error::from))?;
        Self::for_address(ecc_key, None, None)
    }
}

impl Client {
    pub fn get_hotspots(&self, owner: &Pubkey) -> Result<Vec<Hotspot>> {
        let base_params = json!({
            "creatorVerified": true,
            "creatorAddress": "Fv5hf1Fg58htfC7YEXKNEfkpuogUUQDDTLgjGWxxv48H",
            "ownerAddress": owner.to_string(),
        });
        let mut page = 1;
        let mut results = vec![];
        let client = self.settings.mk_jsonrpc_client()?;
        loop {
            let mut params = base_params.clone();
            params["page"] = page.into();
            let page_result: PagedResult = client.call("searchAssets", &[jsonrpc::arg(params)])?;
            if page_result.items.is_empty() {
                break;
            }
            let hotspots: Vec<Hotspot> = page_result.try_into()?;
            results.extend(hotspots);
            page += 1;
        }

        Ok(results)
    }

    fn get_hotspot_info_in_dao(
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
            SubDao::Mobile => maybe_info(
                program.account::<helium_entity_manager::MobileHotspotInfoV0>(hotspot_key),
            ),
        }
    }

    pub fn get_hotspot_info(
        &self,
        subdaos: &[SubDao],
        key: &helium_crypto::PublicKey,
    ) -> Result<Hotspot> {
        let settings = self.settings.clone();
        let (owner, infos) = rayon::join(
            || self.get_hotspot_owner(key),
            || {
                subdaos
                    .par_iter()
                    .filter_map(|subdao| {
                        match Self::get_hotspot_info_in_dao(&settings, subdao, key) {
                            Ok(Some(metadata)) => Some(Ok((*subdao, metadata))),
                            Ok(None) => None,
                            Err(err) => Some(Err(err)),
                        }
                    })
                    .collect::<Result<Vec<(SubDao, HotspotInfo)>>>()
            },
        );
        Hotspot::for_address(key.clone(), Some(owner?), Some(HashMap::from_iter(infos?)))
    }

    pub fn hotspot_has_onboarding_record(
        &self,
        onboarding_server: &str,
        hotspot: &helium_crypto::PublicKey,
    ) -> Result<bool> {
        let client = Settings::mk_rest_client().unwrap();
        let url = format!("{onboarding_server}/hotspots/{hotspot}");
        let resp = client.get(url).send()?;
        Ok(resp.status().is_success())
    }

    pub fn get_hotspot_owner(&self, hotspot_key: &helium_crypto::PublicKey) -> Result<Pubkey> {
        let asset = self.get_hotspot_asset(hotspot_key)?;
        Ok(asset.ownership.owner)
    }

    pub fn hotspot_assert(
        &self,
        onboarding_server: &str,
        subdao: SubDao,
        hotspot: &helium_crypto::PublicKey,
        assertion: HotspotAssertion,
        keypair: Rc<Keypair>,
    ) -> Result<solana_sdk::transaction::Transaction> {
        let client = Settings::mk_rest_client()?;
        let url = format!(
            "{}/transactions/{}/update-metadata",
            onboarding_server, subdao
        );
        let mut params = json!({
            "entityKey": hotspot.to_string(),
            "wallet": keypair.public_key().to_string(),
        });

        if let Some(location) = assertion.location {
            params["location"] = location.into();
        }
        if let Some(gain) = assertion.gain {
            params["gain"] = gain.into();
        }
        if let Some(elevation) = assertion.elevation {
            params["elevation"] = elevation.into();
        }

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
                .map_err(anyhow::Error::from)
            })?;

        tx.try_partial_sign(&[&*keypair], tx.message.recent_blockhash)?;
        Ok(tx)
    }

    /// Entity keys are (regrettably) encoded through the bytes of a the b58
    /// string form of the helium public key
    pub fn hotspot_key_to_entity(&self, hotspot_key: &helium_crypto::PublicKey) -> Result<Vec<u8>> {
        Ok(bs58::decode(hotspot_key.to_string()).into_vec()?)
    }

    pub fn entity_key_to_asset(
        &self,
        entity_key: &[u8],
    ) -> Result<helium_entity_manager::KeyToAssetV0> {
        let client = self.settings.mk_anchor_client(Keypair::void())?;
        let program = client.program(helium_entity_manager::id())?;
        let asset_key = Dao::Hnt.key_to_asset(entity_key);
        let asset_account = program.account::<helium_entity_manager::KeyToAssetV0>(asset_key)?;
        Ok(asset_account)
    }

    pub fn hotspot_key_to_asset(
        &self,
        hotspot_key: &helium_crypto::PublicKey,
    ) -> Result<helium_entity_manager::KeyToAssetV0> {
        let entity_key = self.hotspot_key_to_entity(hotspot_key)?;
        self.entity_key_to_asset(&entity_key)
    }

    pub fn get_hotspot_asset(
        &self,
        hotspot_key: &helium_crypto::PublicKey,
    ) -> Result<AssetResponse> {
        let jsonrpc = self.settings.mk_jsonrpc_client()?;
        let asset_account = self.hotspot_key_to_asset(hotspot_key)?;
        let asset_responase: AssetResponse = jsonrpc
            .call(
                "getAsset",
                &[jsonrpc::arg(json!({
                    "id": asset_account.asset.to_string()
                }))],
            )
            .context("while getting asset")?;
        Ok(asset_responase)
    }

    pub fn get_hotspot_asset_proof(
        &self,
        hotspot_key: &helium_crypto::PublicKey,
    ) -> Result<AsssetProofResponse> {
        let jsonrpc = self.settings.mk_jsonrpc_client()?;
        let asset_account = self.hotspot_key_to_asset(hotspot_key)?;
        let asset_proof_response: AsssetProofResponse = jsonrpc
            .call(
                "getAssetProof",
                &[jsonrpc::arg(json!({
                    "id": asset_account.asset.to_string()
                }))],
            )
            .context("while getting asset proof")?;
        Ok(asset_proof_response)
    }

    pub fn hotspot_dataonly_onboard<C: Clone + Deref<Target = impl Signer> + PublicKey>(
        &self,
        hotspot_key: &helium_crypto::PublicKey,
        assertion: HotspotAssertion,
        keypair: C,
    ) -> Result<solana_sdk::transaction::Transaction> {
        use helium_entity_manager::accounts::OnboardDataOnlyIotHotspotV0;
        fn mk_dataonly_onboard<C: Clone + Deref<Target = impl Signer>>(
            program: &anchor_client::Program<C>,
            entity_key: &[u8],
        ) -> Result<OnboardDataOnlyIotHotspotV0> {
            let dao = Dao::Hnt;
            let sub_dao = SubDao::Iot.key();

            let rewardable_entity_config = SubDao::Iot.rewardable_entity_config_key();
            let iot_info = SubDao::Iot
                .info_key(entity_key)
                .context("Couldn't get iot info key")?;

            let data_only_config = dao.data_only_config_key();
            let data_only_config_acc = program
                .account::<helium_entity_manager::DataOnlyConfigV0>(data_only_config)
                .context(format!(
                    "while getting data only config, {data_only_config}"
                ))?;
            let key_to_asset = dao.key_to_asset(entity_key);
            let dc = SubDao::dc_key();

            Ok(OnboardDataOnlyIotHotspotV0 {
                payer: program.payer(),
                dc_fee_payer: program.payer(),
                iot_info,
                hotspot_owner: program.payer(),
                merkle_tree: data_only_config_acc.merkle_tree,
                dc_burner: get_associated_token_address(&program.payer(), Token::Dc.mint()),
                rewardable_entity_config,
                data_only_config,
                dao: dao.key(),
                key_to_asset,
                sub_dao,
                dc_mint: *Token::Dc.mint(),
                dc,
                compression_program: account_compression_cpi::id(),
                data_credits_program: data_credits::id(),
                helium_sub_daos_program: helium_sub_daos::id(),
                token_program: anchor_spl::token::ID,
                associated_token_program: spl_associated_token_account::id(),
                system_program: system_program::id(),
            })
        }

        let client = self.settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(helium_entity_manager::id())?;

        let entity_key = self.hotspot_key_to_entity(hotspot_key)?;
        let asset = self.get_hotspot_asset(hotspot_key)?;
        let asset_proof = self.get_hotspot_asset_proof(hotspot_key)?;

        let onboard_accounts = mk_dataonly_onboard(&program, &entity_key)
            .context("failed to construct onboarding accounts")?;
        let mut ixs = program
            .request()
            .args(
                helium_entity_manager::instruction::OnboardDataOnlyIotHotspotV0 {
                    args: helium_entity_manager::OnboardDataOnlyIotHotspotArgsV0 {
                        data_hash: asset.compression.data_hash()?,
                        creator_hash: asset.compression.creator_hash()?,
                        index: asset.compression.leaf_id.try_into()?,
                        root: asset_proof.root.to_bytes(),
                        elevation: assertion.elevation,
                        gain: assertion.gain,
                        location: assertion.location,
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

    pub fn hotspot_dataonly_issue(
        &self,
        verifier: &str,
        add_tx: &mut BlockchainTxnAddGatewayV1,
        keypair: Rc<Keypair>,
    ) -> Result<solana_sdk::transaction::Transaction> {
        use helium_entity_manager::accounts::IssueDataOnlyEntityV0;
        fn mk_dataonly_issue<C: Clone + Deref<Target = impl Signer>>(
            program: &anchor_client::Program<C>,
            entity_key: &[u8],
        ) -> Result<IssueDataOnlyEntityV0> {
            use anchor_client::anchor_lang::Id;
            let token_metadata_pid =
                Pubkey::from_str("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s").unwrap();

            let dao = Dao::Hnt;
            let data_only_config = dao.data_only_config_key();
            let data_only_config_acc = program
                .account::<helium_entity_manager::DataOnlyConfigV0>(data_only_config)
                .context(format!(
                    "while getting data only config, {data_only_config}"
                ))?;

            let (collection_metadata, _cm_bump) = Pubkey::find_program_address(
                &[
                    b"metadata",
                    token_metadata_pid.as_ref(),
                    data_only_config_acc.collection.as_ref(),
                ],
                &token_metadata_pid,
            );

            let (collection_master_edition, _cme_bump) = Pubkey::find_program_address(
                &[
                    b"metadata",
                    token_metadata_pid.as_ref(),
                    data_only_config_acc.collection.as_ref(),
                    b"edition",
                ],
                &token_metadata_pid,
            );

            let entity_creator = dao.entity_creator_key();
            let key_to_asset = dao.key_to_asset(entity_key);

            let (tree_authority, _ta_bump) = Pubkey::find_program_address(
                &[data_only_config_acc.merkle_tree.as_ref()],
                &bubblegum_cpi::id(),
            );

            let (data_only_escrow, _doe_bump) = Pubkey::find_program_address(
                &[b"data_only_escrow", data_only_config.as_ref()],
                &helium_entity_manager::id(),
            );

            let (bubblegum_signer, _bs_bump) =
                Pubkey::find_program_address(&[b"collection_cpi"], &bubblegum_cpi::id());

            Ok(IssueDataOnlyEntityV0 {
                payer: program.payer(),
                ecc_verifier: Pubkey::from_str(
                    helium_entity_manager::instructions::issue_entity_v0::ECC_VERIFIER,
                )?,
                collection: data_only_config_acc.collection,
                collection_metadata,
                collection_master_edition,
                data_only_config,
                entity_creator,
                dao: dao.key(),
                key_to_asset,
                tree_authority,
                recipient: program.payer(),
                merkle_tree: data_only_config_acc.merkle_tree,
                data_only_escrow,
                bubblegum_signer,
                token_metadata_program: token_metadata_pid,
                log_wrapper: account_compression_cpi::Noop::id(),
                bubblegum_program: bubblegum_cpi::id(),
                compression_program: account_compression_cpi::id(),
                system_program: system_program::id(),
            })
        }

        let client = self.settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(helium_entity_manager::id())?;
        let hotspot_key = helium_crypto::PublicKey::from_bytes(&add_tx.gateway)?;
        let entity_key = self.hotspot_key_to_entity(&hotspot_key)?;

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

        let signed_tx = self.verify_helium_key(verifier, &msg, &sig, tx)?;
        Ok(signed_tx)
    }
}

#[derive(Debug, Deserialize)]
pub struct AssetResponse {
    pub compression: AssetResponseCompression,
    pub ownership: AssetOwnership,
}

#[derive(Debug, Deserialize)]
pub struct AssetResponseCompression {
    pub data_hash: String,
    pub creator_hash: String,
    pub leaf_id: u64,
}

#[derive(Debug, Deserialize)]
pub struct AssetOwnership {
    #[serde(with = "serde_pubkey")]
    pub owner: Pubkey,
}

impl AssetResponseCompression {
    pub fn data_hash(&self) -> Result<[u8; 32]> {
        Ok(bs58::decode(&self.data_hash)
            .into_vec()?
            .as_slice()
            .try_into()?)
    }
    pub fn creator_hash(&self) -> Result<[u8; 32]> {
        Ok(bs58::decode(&self.creator_hash)
            .into_vec()?
            .as_slice()
            .try_into()?)
    }
}

#[derive(Debug, Deserialize)]
pub struct AsssetProofResponse {
    pub proof: Vec<String>,
    #[serde(with = "serde_pubkey")]
    pub root: Pubkey,
}

impl AsssetProofResponse {
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
