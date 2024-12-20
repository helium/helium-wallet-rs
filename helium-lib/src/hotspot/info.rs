use crate::{
    anchor_client::solana_client::{
        rpc_client::GetConfirmedSignaturesForAddress2Config, rpc_config::RpcTransactionConfig,
    },
    anchor_lang::{AnchorDeserialize, Discriminator},
    client::{GetAnchorAccount, SolanaRpcClient},
    dao::SubDao,
    entity_key::AsEntityKey,
    error::{DecodeError, Error},
    helium_entity_manager::{
        self,
        instruction::{
            OnboardDataOnlyIotHotspotV0, OnboardDataOnlyMobileHotspotV0, OnboardIotHotspotV0,
            OnboardMobileHotspotV0, UpdateIotInfoV0, UpdateMobileInfoV0,
        },
        OnboardDataOnlyIotHotspotArgsV0, OnboardDataOnlyMobileHotspotArgsV0,
        OnboardIotHotspotArgsV0, OnboardMobileHotspotArgsV0, UpdateIotInfoArgsV0,
        UpdateMobileInfoArgsV0,
    },
    hotspot::{CommittedHotspotInfoUpdate, HotspotInfo, HotspotInfoUpdate},
    keypair::Pubkey,
    solana_sdk::{commitment_config::CommitmentConfig, signature::Signature},
};
use chrono::DateTime;
use futures::{
    stream::{self, StreamExt, TryStreamExt},
    TryFutureExt,
};
use serde::{Deserialize, Serialize};
use solana_transaction_status::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, UiInstruction, UiMessage,
    UiParsedInstruction, UiTransactionEncoding,
};
use std::{collections::HashMap, str::FromStr};

pub async fn get<C: GetAnchorAccount>(
    client: &C,
    subdao: SubDao,
    info_key: &Pubkey,
) -> Result<Option<HotspotInfo>, Error> {
    let hotspot_info = match subdao {
        SubDao::Iot => client
            .anchor_account::<helium_entity_manager::IotHotspotInfoV0>(info_key)
            .await?
            .map(Into::into),
        SubDao::Mobile => client
            .anchor_account::<helium_entity_manager::MobileHotspotInfoV0>(info_key)
            .await?
            .map(Into::into),
    };

    Ok(hotspot_info)
}

pub async fn get_many<C: GetAnchorAccount>(
    client: &C,
    subdao: SubDao,
    info_keys: &[Pubkey],
) -> Result<Vec<Option<HotspotInfo>>, Error> {
    fn to_infos<T: Into<HotspotInfo>>(maybe_accounts: Vec<Option<T>>) -> Vec<Option<HotspotInfo>> {
        maybe_accounts
            .into_iter()
            .map(HotspotInfo::from_maybe)
            .collect()
    }
    let accounts = match subdao {
        SubDao::Iot => to_infos(
            client
                .anchor_accounts::<helium_entity_manager::IotHotspotInfoV0>(info_keys)
                .await?,
        ),
        SubDao::Mobile => to_infos(
            client
                .anchor_accounts::<helium_entity_manager::MobileHotspotInfoV0>(info_keys)
                .await?,
        ),
    };
    Ok(accounts)
}

async fn for_entity_key_in_subdao<C: GetAnchorAccount, E: AsEntityKey>(
    client: &C,
    subdao: SubDao,
    entity_key: &E,
) -> Result<Option<HotspotInfo>, Error> {
    let info_key = subdao.info_key(entity_key);
    get(client, subdao, &info_key).await
}

pub async fn for_entity_key<C: GetAnchorAccount>(
    client: &C,
    subdaos: &[SubDao],
    key: &helium_crypto::PublicKey,
) -> Result<HashMap<SubDao, HotspotInfo>, Error> {
    stream::iter(subdaos.to_vec())
        .map(|subdao| {
            for_entity_key_in_subdao(client, subdao, key)
                .map_ok(move |maybe_metadata| maybe_metadata.map(|metadata| (subdao, metadata)))
        })
        .buffer_unordered(10)
        .filter_map(|result| async move { result.transpose() })
        .try_collect::<Vec<(SubDao, HotspotInfo)>>()
        .map_ok(HashMap::from_iter)
        .await
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct HotspotInfoUpdateParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Signature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<Signature>,
    #[serde(skip_serializing_if = "Option::is_none")]
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

pub async fn updates<C: AsRef<SolanaRpcClient>>(
    client: &C,
    account: &Pubkey,
    params: HotspotInfoUpdateParams,
) -> Result<Vec<CommittedHotspotInfoUpdate>, Error> {
    let signatures = client
        .as_ref()
        .get_signatures_for_address_with_config(account, params.into())
        .await?;

    let signature_iter = signatures
        .into_iter()
        .filter(|signature| signature.err.is_none())
        .map(|signature| {
            Signature::from_str(signature.signature.as_str())
                .map_err(DecodeError::from)
                .map_err(Error::from)
        });
    let updates = stream::iter(signature_iter)
        .map_ok(|signature| async move {
            client
                .as_ref()
                .get_transaction_with_config(
                    &signature,
                    RpcTransactionConfig {
                        encoding: Some(UiTransactionEncoding::JsonParsed),
                        commitment: Some(CommitmentConfig::finalized()),
                        max_supported_transaction_version: Some(0),
                    },
                )
                .map_err(Error::from)
                .await
        })
        .try_buffered(5)
        .try_filter_map(|txn| async move {
            CommittedHotspotInfoUpdate::from_transaction(txn).map_err(Error::from)
        })
        .try_collect::<Vec<CommittedHotspotInfoUpdate>>()
        .await?;

    Ok(updates)
}

impl CommittedHotspotInfoUpdate {
    fn from_transaction(
        txn: EncodedConfirmedTransactionWithStatusMeta,
    ) -> Result<Option<Self>, DecodeError> {
        // don't handle failed transactions
        if let Some(meta) = txn.transaction.meta {
            if meta.err.is_some() {
                return Ok(None);
            }
        }
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
        let block = txn.slot;
        let signature = &ui_txn.signatures[0];
        let update = ui_msg
            .instructions
            .into_iter()
            .map(HotspotInfoUpdate::from_ui_instruction)
            .filter(|result| matches!(result, Ok(Some(_v))))
            .collect::<Vec<Result<Option<(Pubkey, HotspotInfoUpdate)>, _>>>()
            .into_iter()
            .collect::<Result<Vec<Option<(Pubkey, HotspotInfoUpdate)>>, _>>()?
            .first()
            .cloned()
            .flatten()
            .map(|(info_key, update)| Self {
                block,
                timestamp,
                signature: signature.clone(),
                info_key,
                update,
            });
        Ok(update)
    }
}

impl HotspotInfoUpdate {
    fn from_ui_instruction(ixn: UiInstruction) -> Result<Option<(Pubkey, Self)>, DecodeError> {
        use solana_transaction_status::UiPartiallyDecodedInstruction;
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
        let mut args = &decoded_data[8..];

        fn get_info_key(
            decoded: &UiPartiallyDecodedInstruction,
            index: usize,
        ) -> Result<Pubkey, DecodeError> {
            let account_str = decoded
                .accounts
                .get(index)
                .ok_or_else(|| DecodeError::other("missing info key in instruction accounts"))?;
            let account = Pubkey::from_str(account_str).map_err(DecodeError::from)?;
            Ok(account)
        }
        match discriminator {
            UpdateMobileInfoV0::DISCRIMINATOR => {
                let info_key = get_info_key(&decoded, 2)?;
                UpdateMobileInfoArgsV0::deserialize(&mut args)
                    .map(Into::into)
                    .map(|v| (info_key, v))
            }
            OnboardMobileHotspotV0::DISCRIMINATOR => {
                let info_key = get_info_key(&decoded, 3)?;
                OnboardMobileHotspotArgsV0::deserialize(&mut args)
                    .map(Self::from)
                    .map(|v| (info_key, v))
            }
            OnboardDataOnlyMobileHotspotV0::DISCRIMINATOR => {
                let info_key = get_info_key(&decoded, 2)?;
                OnboardDataOnlyMobileHotspotArgsV0::deserialize(&mut args)
                    .map(Self::from)
                    .map(|v| (info_key, v))
            }
            OnboardIotHotspotV0::DISCRIMINATOR => {
                let info_key = get_info_key(&decoded, 3)?;
                OnboardIotHotspotArgsV0::deserialize(&mut args)
                    .map(Into::into)
                    .map(|v| (info_key, v))
            }
            UpdateIotInfoV0::DISCRIMINATOR => {
                let info_key = get_info_key(&decoded, 2)?;
                UpdateIotInfoArgsV0::deserialize(&mut args)
                    .map(Into::into)
                    .map(|v| (info_key, v))
            }
            OnboardDataOnlyIotHotspotV0::DISCRIMINATOR => {
                let info_key = get_info_key(&decoded, 2)?;
                OnboardDataOnlyIotHotspotArgsV0::deserialize(&mut args)
                    .map(Into::into)
                    .map(|v| (info_key, v))
            }
            _ => return Ok(None),
        }
        .map(Some)
        .map_err(DecodeError::from)
    }
}
