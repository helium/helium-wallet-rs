use crate::{
    asset,
    dao::SubDao,
    entity_key::{self, AsEntityKey, EntityKeyEncoding},
    keypair::{Keypair, Pubkey, PublicKey},
    programs::SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
    result::{DecodeError, Error, Result},
    settings::Settings,
    token::TokenAmount,
};
use futures::{
    stream::{self, StreamExt, TryStreamExt},
    TryFutureExt,
};
use helium_anchor_gen::{
    circuit_breaker, helium_entity_manager,
    lazy_distributor::{self, OracleConfigV0},
};
use serde::{Deserialize, Serialize};
use solana_program::{instruction::Instruction, system_program};
use solana_sdk::signer::Signer;
use std::{collections::HashMap, ops::Deref};

#[derive(Debug, Serialize, Clone)]
pub struct OracleReward {
    #[serde(with = "crate::keypair::serde_pubkey")]
    oracle: Pubkey,
    index: u16,
    reward: TokenAmount,
}

pub async fn lazy_distributor(
    settings: &Settings,
    subdao: &SubDao,
) -> Result<lazy_distributor::LazyDistributorV0> {
    let client = settings.mk_anchor_client(Keypair::void())?;
    let ld_program = client.program(lazy_distributor::id())?;
    let ld_account = ld_program
        .account::<lazy_distributor::LazyDistributorV0>(subdao.lazy_distributor())
        .await?;
    Ok(ld_account)
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

pub const MAX_CLAIM_BONES: u64 = 207020547945205;

pub async fn claim<C: Clone + Deref<Target = impl Signer>, E>(
    settings: &Settings,
    subdao: &SubDao,
    entity_key_string: &str,
    entity_key_encoding: EntityKeyEncoding,
    keypair: C,
) -> Result<solana_sdk::transaction::Transaction>
where
    E: AsEntityKey,
{
    let entity_key = entity_key::from_string(entity_key_string.to_string(), entity_key_encoding)?;
    let rewards = current(settings, subdao, &entity_key).await?;
    let pending = pending(
        settings,
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

    let client = settings.mk_anchor_client(keypair.clone())?;
    let program = client.program(lazy_distributor::id())?;
    let asset_account = asset::account_for_entity_key(&client, &entity_key).await?;
    let ld_account = lazy_distributor(settings, subdao).await?;

    let mut ixs: Vec<Instruction> = rewards
        .into_iter()
        .map(|reward| {
            let _args = lazy_distributor::SetCurrentRewardsArgsV0 {
                current_rewards: reward.reward.amount,
                oracle_index: reward.index,
            };
            let accounts = lazy_distributor::accounts::SetCurrentRewardsV0 {
                lazy_distributor: subdao.lazy_distributor(),
                payer: program.payer(),
                recipient: subdao.asset_key_to_receipient_key(&asset_account.asset),
                oracle: oracle_reward.oracle,
                system_program: solana_sdk::system_program::id(),
            };
            program
                .request()
                .args(lazy_distributor::instruction::SetCurrentRewardsV0 { _args })
                .accounts(accounts)
                .instructions()
                .map_err(Error::from)
        })
        .collect::<Result<Vec<Vec<_>>>>()?
        .into_iter()
        .flatten()
        .collect();

    let (asset, asset_proof) = futures::try_join!(
        asset::get(settings, &asset_account),
        asset::proof::get(settings, &asset_account)
    )?;

    let _args = lazy_distributor::DistributeCompressionRewardsArgsV0 {
        data_hash: asset.compression.data_hash,
        creator_hash: asset.compression.creator_hash,
        root: asset_proof.root.to_bytes(),
        index: asset.compression.leaf_id()?,
    };
    let accounts = lazy_distributor::accounts::DistributeCompressionRewardsV0 {
        DistributeCompressionRewardsV0common:
            lazy_distributor::accounts::DistributeCompressionRewardsV0Common {
                payer: program.payer(),
                lazy_distributor: lazy_distributor::id(),
                associated_token_program: spl_associated_token_account::id(),
                rewards_mint: *subdao.mint(),
                rewards_escrow: ld_account.rewards_escrow,
                system_program: system_program::ID,
                token_program: anchor_spl::token::ID,
                circuit_breaker_program: circuit_breaker::id(),
                owner: asset.ownership.owner,
                circuit_breaker: lazy_distributor_circuit_breaker(&ld_account),
                recipient: subdao.asset_key_to_receipient_key(&asset_account.asset),
                destination_account: subdao
                    .token()
                    .associated_token_adress(&asset.ownership.owner),
            },
        compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        merkle_tree: asset.compression.tree,
        token_program: anchor_spl::token::ID,
    };
    let mut distribute_ixs = program
        .request()
        .args(lazy_distributor::instruction::DistributeCompressionRewardsV0 { _args })
        .accounts(accounts)
        .instructions()?;

    distribute_ixs[0]
        .accounts
        .extend_from_slice(&asset_proof.proof()?[0..3]);
    ixs.push(distribute_ixs[0].clone());

    let mut tx = solana_sdk::transaction::Transaction::new_with_payer(&ixs, Some(&program.payer()));

    let blockhash = program.rpc().get_latest_blockhash()?;
    tx.try_sign(&[&*keypair], blockhash)?;

    Ok(tx)
}

pub async fn current<E: AsEntityKey>(
    settings: &Settings,
    subdao: &SubDao,
    entity_key: &E,
) -> Result<Vec<OracleReward>> {
    let ld_account = lazy_distributor(settings, subdao).await?;
    let asset = asset::for_entity_key(settings, entity_key).await?;
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
                oracle: oracle.oracle,
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
) -> Result<TokenAmount> {
    #[derive(Debug, Deserialize)]
    struct OracleRewardsResponse {
        #[serde(rename = "currentRewards")]
        current_rewards: serde_json::Value,
    }
    let client = Settings::mk_rest_client()?;
    let response = client
        .get(format!("{oracle}?assetId={asset_id}"))
        .send()
        .await?
        .json::<OracleRewardsResponse>()
        .await?;
    value_to_token_amount(subdao, response.current_rewards)
}

pub async fn pending(
    settings: &Settings,
    subdao: &SubDao,
    entity_key_strings: &[String],
    entity_key_encoding: EntityKeyEncoding,
) -> Result<HashMap<String, OracleReward>> {
    fn for_entity_key(
        bulk_rewards: &HashMap<String, Vec<OracleReward>>,
        entity_key_string: &str,
    ) -> Option<OracleReward> {
        let oracle_rewards = bulk_rewards.get(entity_key_string)?;
        let mut sorted_oracle_rewards = oracle_rewards.clone();
        sorted_oracle_rewards.sort_unstable_by_key(|oracle_reward| oracle_reward.reward.amount);
        Some(sorted_oracle_rewards[sorted_oracle_rewards.len() / 2].to_owned())
    }

    let bulk_rewards = bulk(settings, subdao, entity_key_strings).await?;
    let entity_key_rewards = stream::iter(entity_key_strings)
        .map(Ok::<&String, Error>)
        .and_then(|entity_key_string| async {
            let entity_key =
                entity_key::from_string(entity_key_string.clone(), entity_key_encoding)?;
            let client = settings.mk_anchor_client(Keypair::void())?;
            let asset_account = asset::account_for_entity_key(&client, &entity_key).await?;
            recipient::for_asset_account(&client, subdao, &asset_account)
                .and_then(|maybe_recipient| async move {
                    maybe_recipient
                        .ok_or_else(|| anchor_client::ClientError::AccountNotFound.into())
                })
                .map_ok(|recipient| {
                    for_entity_key(&bulk_rewards, entity_key_string).map(|mut oracle_reward| {
                        oracle_reward.reward.amount =
                            (oracle_reward.reward.amount - recipient.total_rewards).max(0);
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

pub async fn bulk(
    settings: &Settings,
    subdao: &SubDao,
    entity_keys: &[String],
) -> Result<HashMap<String, Vec<OracleReward>>> {
    let ld_account = lazy_distributor(settings, subdao).await?;
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
                            oracle: oracle.oracle,
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
) -> Result<HashMap<String, TokenAmount>> {
    #[derive(Debug, Serialize, Deserialize)]
    struct OracleBulkRewardRequest {
        #[serde(rename = "entityKeys")]
        entity_keys: Vec<String>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct OracleBulkRewardResponse {
        #[serde(rename = "currentRewards")]
        current_rewards: HashMap<String, serde_json::Value>,
    }

    let client = Settings::mk_rest_client()?;
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
        .collect::<Vec<Result<_>>>()
        .into_iter()
        .collect()
}

pub mod recipient {
    use super::*;

    pub async fn for_asset_account<C: Clone + Deref<Target = impl Signer>>(
        client: &anchor_client::Client<C>,
        subdao: &SubDao,
        asset_account: &helium_entity_manager::KeyToAssetV0,
    ) -> Result<Option<lazy_distributor::RecipientV0>> {
        let program = client.program(lazy_distributor::id())?;
        let recipient_key = subdao.asset_key_to_receipient_key(&asset_account.asset);
        match program
            .account::<lazy_distributor::RecipientV0>(recipient_key)
            .await
        {
            Ok(receipient) => Ok(Some(receipient)),
            Err(anchor_client::ClientError::AccountNotFound) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub async fn init<C: Clone + Deref<Target = impl Signer> + PublicKey, E>(
        settings: &Settings,
        subdao: &SubDao,
        entity_key: &E,
        keypair: C,
    ) -> Result<solana_sdk::transaction::Transaction>
    where
        E: AsEntityKey,
    {
        let client = settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(lazy_distributor::id())?;
        let asset_account = asset::account_for_entity_key(&client, entity_key).await?;

        let (asset, asset_proof) = futures::try_join!(
            asset::get(settings, &asset_account),
            asset::proof::get(settings, &asset_account)
        )?;

        let _args = lazy_distributor::InitializeCompressionRecipientArgsV0 {
            data_hash: asset.compression.data_hash,
            creator_hash: asset.compression.creator_hash,
            root: asset_proof.root.to_bytes(),
            index: asset.compression.leaf_id()?,
        };

        let accounts = lazy_distributor::accounts::InitializeCompressionRecipientV0 {
            payer: program.payer(),
            lazy_distributor: subdao.lazy_distributor(),
            recipient: subdao.asset_key_to_receipient_key(&asset.id),
            merkle_tree: asset.compression.tree,
            owner: asset.ownership.owner,
            delegate: asset.ownership.owner,
            compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
            system_program: solana_sdk::system_program::id(),
        };

        let mut ixs = program
            .request()
            .args(lazy_distributor::instruction::InitializeCompressionRecipientV0 { _args })
            .accounts(accounts)
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
}

fn value_to_token_amount(subdao: &SubDao, value: serde_json::Value) -> Result<TokenAmount> {
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
