use crate::{
    asset,
    dao::{Dao, SubDao},
    entity_key::AsEntityKey,
    is_zero,
    keypair::{pubkey, serde_pubkey, Keypair, Pubkey, PublicKey},
    onboarding, priority_fee,
    programs::{MPL_BUBBLEGUM_PROGRAM_ID, SPL_ACCOUNT_COMPRESSION_PROGRAM_ID},
    result::{DecodeError, EncodeError, Error, Result},
    settings::{DasClient, DasSearchAssetsParams, Settings},
    token::Token,
};
use anchor_client::{self, solana_sdk::signer::Signer};
use angry_purple_tiger::AnimalName;
use chrono::Utc;
use futures::{
    stream::{self, StreamExt, TryStreamExt},
    TryFutureExt,
};
use helium_anchor_gen::{
    anchor_lang::ToAccountMetas, data_credits, helium_entity_manager, helium_sub_daos,
};
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use solana_program::instruction::AccountMeta;
use std::{collections::HashMap, ops::Deref, result::Result as StdResult, str::FromStr};

pub const HOTSPOT_CREATOR: Pubkey = pubkey!("Fv5hf1Fg58htfC7YEXKNEfkpuogUUQDDTLgjGWxxv48H");
pub const ECC_VERIFIER: Pubkey = pubkey!("eccSAJM3tq7nQSpQTm8roxv4FPoipCkMsGizW2KBhqZ");

pub async fn for_owner(settings: &Settings, owner: &Pubkey) -> Result<Vec<Hotspot>> {
    let assets = asset::for_owner(settings, &HOTSPOT_CREATOR, owner).await?;
    assets
        .into_iter()
        .map(|asset| Hotspot::try_from(asset).map_err(Error::from))
        .collect::<Result<Vec<Hotspot>>>()
}

pub async fn search(client: &DasClient, params: DasSearchAssetsParams) -> Result<HotspotPage> {
    let asset_page = asset::search(client, params).await?;
    Ok(HotspotPage::try_from(asset_page)?)
}

pub async fn get(settings: &Settings, hotspot_key: &helium_crypto::PublicKey) -> Result<Hotspot> {
    let client = settings.mk_anchor_client(Keypair::void())?;
    let asset_account = asset::account_for_entity_key(&client, hotspot_key).await?;
    let asset = asset::get(settings, &asset_account).await?;
    Ok(asset.try_into()?)
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
    use anchor_client::{
        anchor_lang::{AnchorDeserialize, Discriminator},
        solana_client::{
            rpc_client::GetConfirmedSignaturesForAddress2Config, rpc_config::RpcTransactionConfig,
        },
    };
    use chrono::DateTime;
    use helium_anchor_gen::helium_entity_manager::{
        instruction::{
            OnboardDataOnlyIotHotspotV0, OnboardIotHotspotV0, OnboardMobileHotspotV0,
            UpdateIotInfoV0, UpdateMobileInfoV0,
        },
        OnboardDataOnlyIotHotspotArgsV0, OnboardIotHotspotArgsV0, OnboardMobileHotspotArgsV0,
        UpdateIotInfoArgsV0, UpdateMobileInfoArgsV0,
    };
    use solana_sdk::{commitment_config::CommitmentConfig, signature::Signature};
    use solana_transaction_status::{
        EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, UiInstruction, UiMessage,
        UiParsedInstruction, UiTransactionEncoding,
    };

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
        let info_key = subdao.info_key_for_helium_key(key)?;
        let program = client.program(helium_entity_manager::id())?;
        match subdao {
            SubDao::Iot => maybe_info(
                program
                    .account::<helium_entity_manager::IotHotspotInfoV0>(info_key)
                    .await,
            ),
            SubDao::Mobile => maybe_info(
                program
                    .account::<helium_entity_manager::MobileHotspotInfoV0>(info_key)
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

    #[derive(Serialize, Deserialize, Debug, Default)]
    #[skip_serializing_none]
    pub struct HotspotInfoUpdateParams {
        pub before: Option<Signature>,
        pub until: Option<Signature>,
        pub limit: Option<usize>,
    }

    impl From<HotspotInfoUpdateParams> for GetConfirmedSignaturesForAddress2Config {
        fn from(value: HotspotInfoUpdateParams) -> Self {
            Self {
                before: value.before,
                until: value.until,
                limit: value.limit,
                ..Default::default()
            }
        }
    }

    pub async fn updates(
        settings: &Settings,
        subdao: SubDao,
        key: &helium_crypto::PublicKey,
        params: HotspotInfoUpdateParams,
    ) -> Result<Vec<ConfirmedHotspotInfoUpdate>> {
        let info_key = subdao.info_key_for_helium_key(key)?;
        let client = settings.mk_solana_client()?;
        let signatures = client
            .get_signatures_for_address_with_config(&info_key, params.into())
            .await?;

        let updates = stream::iter(signatures.iter())
            .filter(|signature| async { signature.err.is_none() })
            .map(Ok)
            .and_then(|signature| async {
                Signature::from_str(signature.signature.as_str())
                    .map_err(DecodeError::from)
                    .map_err(Error::from)
            })
            .map_ok(|signature| async move {
                let client = settings.mk_solana_client()?;
                client
                    .get_transaction_with_config(
                        &signature,
                        RpcTransactionConfig {
                            encoding: Some(UiTransactionEncoding::JsonParsed),
                            commitment: Some(CommitmentConfig::confirmed()),
                            max_supported_transaction_version: Some(0),
                        },
                    )
                    .map_err(Error::from)
                    .await
            })
            .try_buffered(5)
            .try_filter_map(|txn| async move {
                ConfirmedHotspotInfoUpdate::from_confirmed_transaction(txn).map_err(Error::from)
            })
            .try_collect::<Vec<ConfirmedHotspotInfoUpdate>>()
            .await?;

        Ok(updates)
    }

    impl ConfirmedHotspotInfoUpdate {
        fn from_confirmed_transaction(
            txn: EncodedConfirmedTransactionWithStatusMeta,
        ) -> StdResult<Option<Self>, DecodeError> {
            let EncodedTransaction::Json(ui_txn) = txn.transaction.transaction else {
                return Err(DecodeError::other("not a json encoded transaction"));
            };
            let UiMessage::Parsed(ui_msg) = ui_txn.message else {
                return Err(DecodeError::other("not a parsed transaction message"));
            };
            let Some(timestamp) = txn
                .block_time
                .and_then(|block_time| DateTime::from_timestamp(block_time, 0))
            else {
                return Err(DecodeError::other("no valid block time found"));
            };
            let signature = &ui_txn.signatures[0];
            let update = ui_msg
                .instructions
                .into_iter()
                .map(HotspotInfoUpdate::from_ui_instruction)
                .filter(|result| matches!(result, Ok(Some(_v))))
                .collect::<Vec<StdResult<Option<HotspotInfoUpdate>, _>>>()
                .into_iter()
                .collect::<StdResult<Vec<Option<HotspotInfoUpdate>>, _>>()?
                .first()
                .cloned()
                .flatten()
                .map(|update| Self {
                    timestamp,
                    signature: signature.clone(),
                    update,
                });
            Ok(update)
        }
    }

    impl HotspotInfoUpdate {
        fn from_ui_instruction(ixn: UiInstruction) -> StdResult<Option<Self>, DecodeError> {
            let UiInstruction::Parsed(UiParsedInstruction::PartiallyDecoded(decoded)) = ixn else {
                return Err(DecodeError::other("not a decoded instruction"));
            };
            if decoded.program_id != helium_entity_manager::id().to_string() {
                return Ok(None);
            }
            if decoded.data.is_empty() {
                return Ok(None);
            }
            let decoded_data = solana_sdk::bs58::decode(decoded.data.clone()).into_vec()?;
            if decoded_data.len() < 9 {
                return Ok(None);
            }
            let mut discriminator: [u8; 8] = Default::default();
            discriminator.copy_from_slice(&decoded_data[..8]);
            let args = &decoded_data[8..];

            match discriminator {
                UpdateMobileInfoV0::DISCRIMINATOR => {
                    UpdateMobileInfoArgsV0::try_from_slice(args).map(Into::into)
                }
                OnboardMobileHotspotV0::DISCRIMINATOR => {
                    OnboardMobileHotspotArgsV0::try_from_slice(args).map(Into::into)
                }
                OnboardIotHotspotV0::DISCRIMINATOR => {
                    OnboardIotHotspotArgsV0::try_from_slice(args).map(Into::into)
                }
                UpdateIotInfoV0::DISCRIMINATOR => {
                    UpdateIotInfoArgsV0::try_from_slice(args).map(Into::into)
                }
                OnboardDataOnlyIotHotspotV0::DISCRIMINATOR => {
                    OnboardDataOnlyIotHotspotArgsV0::try_from_slice(args).map(Into::into)
                }
                _ => return Ok(None),
            }
            .map(Some)
            .map_err(DecodeError::from)
        }
    }
}

pub async fn direct_update<C: Clone + Deref<Target = impl Signer> + PublicKey>(
    settings: &Settings,
    hotspot: &helium_crypto::PublicKey,
    keypair: C,
    update: HotspotInfoUpdate,
) -> Result<solana_sdk::transaction::Transaction> {
    fn mk_update_accounts(
        subdao: SubDao,
        asset_account: &helium_entity_manager::KeyToAssetV0,
        asset: &asset::Asset,
        owner: &Pubkey,
    ) -> Vec<AccountMeta> {
        use helium_entity_manager::accounts::{UpdateIotInfoV0, UpdateMobileInfoV0};
        macro_rules! mk_update_info {
            ($name:ident, $info:ident) => {
                $name {
                    bubblegum_program: MPL_BUBBLEGUM_PROGRAM_ID,
                    payer: owner.to_owned(),
                    dc_fee_payer: owner.to_owned(),
                    $info: subdao.info_key(&asset_account.entity_key),
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

    let anchor_client = settings.mk_anchor_client(keypair.clone())?;
    let program = anchor_client.program(helium_entity_manager::id())?;
    let solana_client = settings.mk_solana_client()?;

    let asset_account = asset::account_for_entity_key(&anchor_client, hotspot).await?;
    let asset = asset::get(settings, &asset_account).await?;
    let asset_proof = asset::proof::get(settings, &asset_account).await?;

    let update_accounts =
        mk_update_accounts(update.subdao(), &asset_account, &asset, &program.payer());
    let priority_fee = priority_fee::get_estimate(
        &solana_client,
        &update_accounts,
        priority_fee::MIN_PRIORITY_FEE,
    )
    .await?;

    let compute_ix =
        solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(200_000);
    let compute_price_ix =
        solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_price(priority_fee);
    let mut ixs = program
        .request()
        .instruction(compute_ix)
        .instruction(compute_price_ix)
        .args(helium_entity_manager::instruction::UpdateIotInfoV0 {
            _args: helium_entity_manager::UpdateIotInfoArgsV0 {
                root: asset_proof.root.to_bytes(),
                data_hash: asset.compression.data_hash,
                creator_hash: asset.compression.creator_hash,
                index: asset.compression.leaf_id()?,
                elevation: *update.elevation(),
                gain: update.gain_i32(),
                location: update.location_u64(),
            },
        })
        .accounts(update_accounts)
        .instructions()?;
    ixs[2]
        .accounts
        .extend_from_slice(&asset_proof.proof()?[0..3]);

    let mut tx = solana_sdk::transaction::Transaction::new_with_payer(&ixs, Some(&program.payer()));
    let blockhash = program.rpc().get_latest_blockhash()?;
    tx.try_sign(&[&*keypair], blockhash)?;
    Ok(tx)
}

pub async fn update<C: Clone + Deref<Target = impl Signer> + PublicKey>(
    settings: &Settings,
    onboarding_server: Option<String>,
    hotspot: &helium_crypto::PublicKey,
    update: HotspotInfoUpdate,
    keypair: C,
) -> Result<solana_sdk::transaction::Transaction> {
    let public_key = keypair.public_key();
    if let Some(server) = onboarding_server {
        let onboarding_client = onboarding::Client::new(&server);
        let mut tx = onboarding_client
            .get_update_txn(hotspot, &public_key, update)
            .await?;
        tx.try_partial_sign(&[&*keypair], tx.message.recent_blockhash)?;
        return Ok(tx);
    };
    let mut tx = direct_update(settings, hotspot, keypair.clone(), update).await?;
    tx.try_partial_sign(&[&*keypair], tx.message.recent_blockhash)?;
    Ok(tx)
}

pub mod dataonly {
    use super::*;
    use crate::programs::{
        SPL_ACCOUNT_COMPRESSION_PROGRAM_ID, SPL_NOOP_PROGRAM_ID, TOKEN_METADATA_PROGRAM_ID,
    };
    use helium_proto::{BlockchainTxnAddGatewayV1, Message};

    pub async fn onboard<C: Clone + Deref<Target = impl Signer> + PublicKey>(
        settings: &Settings,
        hotspot_key: &helium_crypto::PublicKey,
        assertion: HotspotInfoUpdate,
        keypair: C,
    ) -> Result<solana_sdk::transaction::Transaction> {
        use helium_entity_manager::accounts::OnboardDataOnlyIotHotspotV0;
        fn mk_onboard_accounts(
            config_account: helium_entity_manager::DataOnlyConfigV0,
            owner: Pubkey,
            hotspot_key: &helium_crypto::PublicKey,
        ) -> OnboardDataOnlyIotHotspotV0 {
            let dao = Dao::Hnt;
            let entity_key = hotspot_key.as_entity_key();
            let data_only_config_key = dao.dataonly_config_key();

            OnboardDataOnlyIotHotspotV0 {
                payer: owner,
                dc_fee_payer: owner,
                iot_info: SubDao::Iot.info_key(&entity_key),
                hotspot_owner: owner,
                merkle_tree: config_account.merkle_tree,
                dc_burner: Token::Dc.associated_token_adress(&owner),
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
            }
        }

        let anchor_client = settings.mk_anchor_client(keypair.clone())?;
        let solana_client = settings.mk_solana_client()?;
        let program = anchor_client.program(helium_entity_manager::id())?;
        let config_account = program
            .account::<helium_entity_manager::DataOnlyConfigV0>(Dao::Hnt.dataonly_config_key())
            .await?;

        let asset_account = asset::account_for_entity_key(&anchor_client, hotspot_key).await?;
        let asset = asset::get(settings, &asset_account).await?;
        let asset_proof = asset::proof::get(settings, &asset_account).await?;

        let onboard_accounts = mk_onboard_accounts(config_account, program.payer(), hotspot_key);
        let compute_ix =
            solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(300_000);
        let priority_fee = priority_fee::get_estimate(
            &solana_client,
            &onboard_accounts,
            priority_fee::MIN_PRIORITY_FEE,
        )
        .await?;
        let compute_price_ix =
            solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_price(
                priority_fee,
            );
        let mut ixs = program
            .request()
            .args(
                helium_entity_manager::instruction::OnboardDataOnlyIotHotspotV0 {
                    _args: helium_entity_manager::OnboardDataOnlyIotHotspotArgsV0 {
                        data_hash: asset.compression.data_hash,
                        creator_hash: asset.compression.creator_hash,
                        index: asset.compression.leaf_id()?,
                        root: asset_proof.root.to_bytes(),
                        elevation: *assertion.elevation(),
                        gain: assertion.gain_i32(),
                        location: assertion.location_u64(),
                    },
                },
            )
            .accounts(onboard_accounts)
            .instruction(compute_ix)
            .instruction(compute_price_ix)
            .instructions()?;
        ixs[2]
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
        fn mk_issue_accounts(
            config_account: helium_entity_manager::DataOnlyConfigV0,
            owner: Pubkey,
            entity_key: &[u8],
        ) -> IssueDataOnlyEntityV0 {
            let dao = Dao::Hnt;
            let dataonly_config_key = dao.dataonly_config_key();
            IssueDataOnlyEntityV0 {
                payer: owner,
                ecc_verifier: ECC_VERIFIER,
                collection: config_account.collection,
                collection_metadata: dao.collection_metadata_key(&config_account.collection),
                collection_master_edition: dao
                    .collection_master_edition_key(&config_account.collection),
                data_only_config: dataonly_config_key,
                entity_creator: dao.entity_creator_key(),
                dao: dao.key(),
                key_to_asset: dao.key_to_asset_key(&entity_key),
                tree_authority: dao.merkle_tree_authority(&config_account.merkle_tree),
                recipient: owner,
                merkle_tree: config_account.merkle_tree,
                data_only_escrow: dao.dataonly_escrow_key(),
                bubblegum_signer: dao.bubblegum_signer(),
                token_metadata_program: TOKEN_METADATA_PROGRAM_ID,
                log_wrapper: SPL_NOOP_PROGRAM_ID,
                bubblegum_program: MPL_BUBBLEGUM_PROGRAM_ID,
                compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                system_program: solana_sdk::system_program::id(),
            }
        }

        let anchor_client = settings.mk_anchor_client(keypair.clone())?;
        let solana_client = settings.mk_solana_client()?;
        let program = anchor_client.program(helium_entity_manager::id())?;
        let config_account = program
            .account::<helium_entity_manager::DataOnlyConfigV0>(Dao::Hnt.dataonly_config_key())
            .await?;
        let hotspot_key = helium_crypto::PublicKey::from_bytes(&add_tx.gateway)?;
        let entity_key = hotspot_key.as_entity_key();

        let issue_accounts = mk_issue_accounts(config_account, program.payer(), &entity_key);
        let compute_ix =
            solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(300_000);
        let priority_fee = priority_fee::get_estimate(
            &solana_client,
            &issue_accounts,
            priority_fee::MIN_PRIORITY_FEE,
        )
        .await?;
        let compute_price_ix =
            solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_price(
                priority_fee,
            );

        let ix = program
            .request()
            .args(helium_entity_manager::instruction::IssueDataOnlyEntityV0 {
                _args: helium_entity_manager::IssueDataOnlyEntityArgsV0 { entity_key },
            })
            .accounts(issue_accounts)
            .instruction(compute_ix)
            .instruction(compute_price_ix)
            .instructions()?;

        let mut tx =
            solana_sdk::transaction::Transaction::new_with_payer(&ix, Some(&keypair.public_key()));
        let blockhash = program.rpc().get_latest_blockhash()?;

        tx.try_partial_sign(&[&*keypair], blockhash)?;

        let sig = add_tx.gateway_signature.clone();
        add_tx.gateway_signature = vec![];
        let msg = add_tx.encode_to_vec();

        let signed_tx = verify_helium_key(verifier, &msg, &sig, tx).await?;
        Ok(signed_tx)
    }

    async fn verify_helium_key(
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
            .send()
            .await?
            .json::<VerifyResponse>()
            .await?;
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
    type Error = DecodeError;
    fn try_from(value: asset::AssetPage) -> StdResult<Self, Self::Error> {
        Ok(Self {
            total: value.total,
            limit: value.limit,
            page: value.page,
            items: value
                .items
                .into_iter()
                .map(Hotspot::try_from)
                .collect::<StdResult<Vec<Hotspot>, DecodeError>>()?,
        })
    }
}

#[derive(Debug, Serialize, Clone)]
#[skip_serializing_none]
pub struct Hotspot {
    pub key: helium_crypto::PublicKey,
    pub name: String,
    #[serde(with = "serde_pubkey")]
    pub owner: Pubkey,
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

#[derive(Serialize, Debug, Clone, Copy)]
pub struct HotspotGeo {
    lat: f64,
    lng: f64,
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
    #[serde(with = "CellIndexHex")]
    location: h3o::CellIndex,
    geo: HotspotGeo,
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
    fn try_from(value: u64) -> StdResult<Self, Self::Error> {
        h3o::CellIndex::try_from(value).map(Into::into)
    }
}

impl HotspotLocation {
    pub fn from_maybe<T: TryInto<HotspotLocation>>(value: Option<T>) -> Option<Self> {
        value.and_then(|v| TryInto::try_into(v).ok())
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase", untagged)]
#[skip_serializing_none]
pub enum HotspotInfo {
    Iot {
        asset: Option<String>,
        mode: HotspotMode,
        gain: Option<Decimal>,
        elevation: Option<i32>,
        #[serde(flatten)]
        location: Option<HotspotLocation>,
        #[serde(skip_serializing_if = "is_zero")]
        location_asserts: u16,
    },
    Mobile {
        asset: Option<String>,
        mode: HotspotMode,
        #[serde(flatten)]
        location: Option<HotspotLocation>,
        #[serde(skip_serializing_if = "is_zero")]
        location_asserts: u16,
        device_type: MobileDeviceType,
    },
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub struct ConfirmedHotspotInfoUpdate {
    timestamp: chrono::DateTime<Utc>,
    signature: String,
    update: HotspotInfoUpdate,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase", untagged)]
#[skip_serializing_none]
pub enum HotspotInfoUpdate {
    Iot {
        gain: Option<Decimal>,
        elevation: Option<i32>,
        #[serde(flatten)]
        location: Option<HotspotLocation>,
    },
    Mobile {
        #[serde(flatten)]
        location: Option<HotspotLocation>,
    },
}

serde_with::serde_conv!(
    CellIndexHex,
    h3o::CellIndex,
    |index: &h3o::CellIndex| { index.to_string() },
    |value: &str| -> StdResult<_, h3o::error::InvalidCellIndex> { value.parse() }
);

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

    pub fn set_geo(self, lat: Option<f64>, lon: Option<f64>) -> StdResult<Self, EncodeError> {
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
            asset: Some(value.asset.to_string()),
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

impl TryFrom<asset::Asset> for Hotspot {
    type Error = DecodeError;
    fn try_from(value: asset::Asset) -> StdResult<Self, Self::Error> {
        value
            .content
            .metadata
            .get_attribute("ecc_compact")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DecodeError::other("no entity key found"))
            .and_then(|str| helium_crypto::PublicKey::from_str(str).map_err(DecodeError::from))
            .map(|hotspot_key| Self::with_hotspot_key(hotspot_key, value.ownership.owner))
    }
}
