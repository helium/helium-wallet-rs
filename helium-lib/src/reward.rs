use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    asset, circuit_breaker,
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    dao::SubDao,
    entity_key::{self, AsEntityKey, KeySerialization},
    error::{DecodeError, Error},
    helium_entity_manager,
    keypair::{Keypair, Pubkey},
    kta,
    lazy_distributor::{self, OracleConfigV0},
    priority_fee,
    programs::SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
    solana_sdk::{instruction::Instruction, transaction::Transaction},
    token::TokenAmount,
};
use futures::{
    stream::{self, StreamExt, TryStreamExt},
    TryFutureExt,
};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use solana_sdk::signer::Signer;
use std::collections::HashMap;

#[derive(Debug, Serialize, Clone)]
pub struct Oracle {
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub key: Pubkey,
    pub url: String,
}

impl From<OracleConfigV0> for Oracle {
    fn from(value: OracleConfigV0) -> Self {
        Self {
            key: value.oracle,
            url: value.url,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct OracleReward {
    oracle: Oracle,
    index: u16,
    reward: TokenAmount,
}

pub async fn lazy_distributor<C: GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
) -> Result<lazy_distributor::LazyDistributorV0, Error> {
    client
        .anchor_account::<lazy_distributor::LazyDistributorV0>(&subdao.lazy_distributor())
        .await
}

pub fn lazy_distributor_circuit_breaker(
    ld_account: &lazy_distributor::LazyDistributorV0,
) -> Pubkey {
    let (circuit_breaker, _) = Pubkey::find_program_address(
        &[
            b"account_windowed_breaker",
            ld_account.rewards_escrow.as_ref(),
        ],
        &circuit_breaker::id(),
    );
    circuit_breaker
}

pub async fn max_claim<C: GetAnchorAccount>(
    client: &C,
    ld_account: &lazy_distributor::LazyDistributorV0,
) -> Result<circuit_breaker::WindowedCircuitBreakerConfigV0, Error> {
    let circuit_breaker_account: circuit_breaker::AccountWindowedCircuitBreakerV0 = client
        .anchor_account(&lazy_distributor_circuit_breaker(ld_account))
        .await?;
    Ok(circuit_breaker_account.config)
}

pub async fn claim<C: AsRef<DasClient> + GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
    entity_key_string: &str,
    entity_key_encoding: KeySerialization,
    keypair: &Keypair,
) -> Result<Transaction, Error> {
    let entity_key = entity_key::from_str(entity_key_string, entity_key_encoding)?;
    let rewards = current(client, subdao, &entity_key).await?;
    let pending = pending(
        client,
        subdao,
        &[entity_key_string.to_string()],
        entity_key_encoding,
    )
    .await?;

    let oracle_reward = pending.get(entity_key_string).ok_or_else(|| {
        DecodeError::other(format!(
            "entity key: {entity_key_string} has no pending rewards"
        ))
    })?;

    let kta = kta::for_entity_key(&entity_key).await?;
    let ld_account = lazy_distributor(client, subdao).await?;

    let mut ixs: Vec<Instruction> = rewards
        .into_iter()
        .map(|reward| {
            let accounts = lazy_distributor::accounts::SetCurrentRewardsV0 {
                lazy_distributor: subdao.lazy_distributor(),
                payer: keypair.pubkey(),
                recipient: subdao.receipient_key_from_kta(&kta),
                oracle: oracle_reward.oracle.key,
                system_program: solana_sdk::system_program::id(),
            }
            .to_account_metas(None);
            Instruction {
                program_id: lazy_distributor::id(),
                accounts,
                data: lazy_distributor::instruction::SetCurrentRewardsV0 {
                    _args: lazy_distributor::SetCurrentRewardsArgsV0 {
                        current_rewards: reward.reward.amount,
                        oracle_index: reward.index,
                    },
                }
                .data(),
            }
        })
        .collect();

    let (asset, asset_proof) = asset::for_kta_with_proof(client, &kta).await?;

    let accounts = lazy_distributor::accounts::DistributeCompressionRewardsV0 {
        DistributeCompressionRewardsV0common:
            lazy_distributor::accounts::DistributeCompressionRewardsV0Common {
                payer: keypair.pubkey(),
                lazy_distributor: lazy_distributor::id(),
                associated_token_program: spl_associated_token_account::id(),
                rewards_mint: *subdao.mint(),
                rewards_escrow: ld_account.rewards_escrow,
                system_program: solana_sdk::system_program::ID,
                token_program: anchor_spl::token::ID,
                circuit_breaker_program: circuit_breaker::id(),
                owner: asset.ownership.owner,
                circuit_breaker: lazy_distributor_circuit_breaker(&ld_account),
                recipient: subdao.receipient_key_from_kta(&kta),
                destination_account: subdao
                    .token()
                    .associated_token_adress(&asset.ownership.owner),
            },
        compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        merkle_tree: asset.compression.tree,
    };

    let mut distribute_ix = Instruction {
        accounts: accounts.to_account_metas(None),
        program_id: lazy_distributor::id(),
        data: lazy_distributor::instruction::DistributeCompressionRewardsV0 {
            _args: lazy_distributor::DistributeCompressionRewardsArgsV0 {
                data_hash: asset.compression.data_hash,
                creator_hash: asset.compression.creator_hash,
                root: asset_proof.root.to_bytes(),
                index: asset.compression.leaf_id()?,
            },
        }
        .data(),
    };

    distribute_ix
        .accounts
        .extend_from_slice(&asset_proof.proof(Some(3))?);
    ixs.push(distribute_ix);

    let mut tx = solana_sdk::transaction::Transaction::new_with_payer(&ixs, Some(&program.payer()));

    let blockhash = program.rpc().get_latest_blockhash()?;
    tx.try_sign(&[&*keypair], blockhash)?;

    Ok(tx)
}

pub async fn current<E: AsEntityKey, C: AsRef<DasClient> + GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
    entity_key: &E,
) -> Result<Vec<OracleReward>, Error> {
    let ld_account = lazy_distributor(client, subdao).await?;
    let asset = asset::for_entity_key(client, entity_key).await?;
    stream::iter(
        ld_account
            .oracles
            .into_iter()
            .enumerate()
            .collect::<Vec<(usize, OracleConfigV0)>>(),
    )
    .map(|(index, oracle): (usize, OracleConfigV0)| async move {
        current_from_oracle(subdao, &oracle.url, &asset.id)
            .map_ok(|reward| OracleReward {
                reward,
                oracle: oracle.clone().into(),
                index: index as u16,
            })
            .await
    })
    .buffered(2)
    .try_collect()
    .await
}

async fn current_from_oracle(
    subdao: &SubDao,
    oracle: &str,
    asset_id: &Pubkey,
) -> Result<TokenAmount, Error> {
    #[derive(Debug, Deserialize)]
    struct OracleRewardsResponse {
        #[serde(rename = "currentRewards")]
        current_rewards: serde_json::Value,
    }
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{oracle}?assetId={asset_id}"))
        .send()
        .await?
        .json::<OracleRewardsResponse>()
        .await?;
    value_to_token_amount(subdao, response.current_rewards)
}

pub async fn pending<C: GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
    entity_key_strings: &[String],
    entity_key_encoding: KeySerialization,
) -> Result<HashMap<String, OracleReward>, Error> {
    fn for_entity_key(
        bulk_rewards: &HashMap<String, Vec<OracleReward>>,
        entity_key_string: &str,
    ) -> Option<OracleReward> {
        let oracle_rewards = bulk_rewards.get(entity_key_string)?;
        let mut sorted_oracle_rewards = oracle_rewards.clone();
        sorted_oracle_rewards.sort_unstable_by_key(|oracle_reward| oracle_reward.reward.amount);
        Some(sorted_oracle_rewards.remove(sorted_oracle_rewards.len() / 2))
    }

    let bulk_rewards = bulk(client, subdao, entity_key_strings).await?;
    let entity_key_rewards = stream::iter(entity_key_strings)
        .map(Ok::<&String, Error>)
        .and_then(|entity_key_string| async {
            let entity_key = entity_key::from_str(&entity_key_string.clone(), entity_key_encoding)?;
            let kta = kta::for_entity_key(&entity_key).await?;
            recipient::for_kta(client, subdao, &kta)
                .and_then(|maybe_recipient| async move {
                    maybe_recipient.ok_or_else(Error::account_not_found)
                })
                .map_ok(|recipient| {
                    for_entity_key(&bulk_rewards, entity_key_string).map(|mut oracle_reward| {
                        oracle_reward.reward.amount = oracle_reward
                            .reward
                            .amount
                            .saturating_sub(recipient.total_rewards);
                        (entity_key_string.clone(), oracle_reward)
                    })
                })
                .await
        })
        // TODO: used buffered after collecting a vec of futures above.
        // The problem has been the various error responses in the and_then block above
        .try_collect::<Vec<Option<(String, OracleReward)>>>()
        .await?
        .into_iter()
        .flatten()
        .collect();

    Ok(entity_key_rewards)
}

pub async fn bulk<C: GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
    entity_keys: &[String],
) -> Result<HashMap<String, Vec<OracleReward>>, Error> {
    let ld_account = lazy_distributor(client, subdao).await?;
    stream::iter(ld_account.oracles)
        .enumerate()
        .map(Ok)
        .try_fold(
            HashMap::new(),
            |mut result, (index, oracle): (usize, OracleConfigV0)| async move {
                let bulk_rewards = bulk_from_oracle(subdao, &oracle.url, entity_keys).await?;
                bulk_rewards
                    .into_iter()
                    .for_each(|(entity_key, token_amount)| {
                        let oracle_reward = OracleReward {
                            oracle: oracle.clone().into(),
                            index: index as u16,
                            reward: token_amount,
                        };
                        let entity_rewards: &mut Vec<_> = result.entry(entity_key).or_default();
                        entity_rewards.push(oracle_reward);
                    });
                Ok(result)
            },
        )
        .await
}

async fn bulk_from_oracle(
    subdao: &SubDao,
    oracle: &str,
    entity_keys: &[String],
) -> Result<HashMap<String, TokenAmount>, Error> {
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OracleBulkRewardRequest {
        entity_keys: Vec<String>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OracleBulkRewardResponse {
        current_rewards: HashMap<String, serde_json::Value>,
    }

    let client = reqwest::Client::new();
    let oracle_rewards_response = client
        .post(format!("{oracle}/bulk-rewards"))
        .json(&OracleBulkRewardRequest {
            entity_keys: entity_keys.into(),
        })
        .send()
        .await?
        .json::<OracleBulkRewardResponse>()
        .await?;
    oracle_rewards_response
        .current_rewards
        .into_iter()
        .map(|(entity_key_string, value)| {
            value_to_token_amount(subdao, value).map(|amount| (entity_key_string, amount))
        })
        .try_collect()
}

pub mod recipient {
    use super::*;

    pub async fn for_kta<C: GetAnchorAccount>(
        client: &C,
        subdao: &SubDao,
        kta: &helium_entity_manager::KeyToAssetV0,
    ) -> Result<Option<lazy_distributor::RecipientV0>, Error> {
        let recipient_key = subdao.receipient_key_from_kta(kta);
        Ok(client.anchor_account(&recipient_key).await.ok())
    }

    pub async fn init_instruction<E: AsEntityKey, C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
        asset: &asset::Asset,
        asset_proof: &asset::AssetProof,
        subdao: &SubDao,
        entity_key: &E,
        keypair: &Keypair,
    ) -> Result<Instruction, Error> {
        fn mk_accounts(
            payer: Pubkey,
            owner: Pubkey,
            tree: Pubkey,
            subdao: &SubDao,
            kta: &helium_entity_manager::KeyToAssetV0,
        ) -> impl ToAccountMetas {
            lazy_distributor::accounts::InitializeCompressionRecipientV0 {
                payer,
                lazy_distributor: subdao.lazy_distributor(),
                recipient: subdao.receipient_key_from_kta(kta),
                merkle_tree: tree,
                owner,
                delegate: owner,
                compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                system_program: solana_sdk::system_program::id(),
            }
        }

        let kta = kta::for_entity_key(entity_key).await?;
        let mut accounts = mk_accounts(
            keypair.pubkey(),
            asset.ownership.owner,
            asset.compression.tree,
            subdao,
            &kta,
        )
        .to_account_metas(None);
        accounts.extend_from_slice(&asset_proof.proof(Some(3))?);

        let init_ix = Instruction {
            program_id: lazy_distributor::id(),
            accounts: accounts.to_account_metas(None),
            data: lazy_distributor::instruction::InitializeCompressionRecipientV0 {
                _args: lazy_distributor::InitializeCompressionRecipientArgsV0 {
                    data_hash: asset.compression.data_hash,
                    creator_hash: asset.compression.creator_hash,
                    root: asset_proof.root.to_bytes(),
                    index: asset.compression.leaf_id()?,
                },
            }
            .data(),
        };
        Ok(init_ix)
    }
}

fn value_to_token_amount(subdao: &SubDao, value: serde_json::Value) -> Result<TokenAmount, Error> {
    let value = match value {
        serde_json::Value::String(s) => s
            .parse::<u64>()
            .map_err(|_| DecodeError::other(format!("invalid reward value {s}")))?,
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| DecodeError::other(format!("invalid reward value {n}")))?,
        _ => return Err(DecodeError::other(format!("invalid reward value {value}")).into()),
    };

    Ok(TokenAmount::from_u64(subdao.token(), value))
}
