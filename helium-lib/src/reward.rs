use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    asset, circuit_breaker,
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    dao::{Dao, SubDao},
    entity_key::{self, AsEntityKey, KeySerialization},
    error::EncodeError,
    error::{DecodeError, Error},
    helium_entity_manager,
    keypair::{Keypair, Pubkey},
    kta, lazy_distributor, priority_fee,
    programs::SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
    rewards_oracle,
    solana_sdk::{instruction::Instruction, transaction::Transaction},
    token::TokenAmount,
};
use anchor_client::solana_client::rpc_client::SerializableTransaction;
use futures::{
    stream::{self, StreamExt, TryStreamExt},
    TryFutureExt,
};
use itertools::{izip, Itertools};
use serde::{Deserialize, Serialize};
use solana_sdk::signer::Signer;
use std::collections::HashMap;

#[derive(Debug, Serialize, Clone)]
pub struct Oracle {
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub key: Pubkey,
    pub url: String,
}

impl From<lazy_distributor::OracleConfigV0> for Oracle {
    fn from(value: lazy_distributor::OracleConfigV0) -> Self {
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
    subdao: &SubDao,
    ld_account: &lazy_distributor::LazyDistributorV0,
) -> Result<TokenAmount, Error> {
    let circuit_breaker_account: circuit_breaker::AccountWindowedCircuitBreakerV0 = client
        .anchor_account(&lazy_distributor_circuit_breaker(ld_account))
        .await?;
    let amount = match circuit_breaker_account.config {
        circuit_breaker::WindowedCircuitBreakerConfigV0 {
            threshold_type: circuit_breaker::ThresholdType::Absolute,
            threshold,
            ..
        } => subdao.token().amount(threshold),
        _ => return Err(DecodeError::other("percent max claim threshold not supported").into()),
    };
    Ok(amount)
}

async fn set_current_rewards_instruction(
    subdao: &SubDao,
    kta_key: Pubkey,
    kta: &helium_entity_manager::KeyToAssetV0,
    reward: &OracleReward,
) -> Result<Instruction, Error> {
    let accounts = rewards_oracle::accounts::SetCurrentRewardsWrapperV1 {
        oracle: reward.oracle.key,
        lazy_distributor: subdao.lazy_distributor(),
        recipient: subdao.receipient_key_from_kta(kta),
        lazy_distributor_program: lazy_distributor::id(),
        system_program: solana_sdk::system_program::id(),
        key_to_asset: kta_key,
        oracle_signer: Dao::oracle_signer_key(),
    }
    .to_account_metas(None);

    let ix = Instruction {
        program_id: rewards_oracle::id(),
        accounts,
        data: rewards_oracle::instruction::SetCurrentRewardsWrapperV1 {
            _args: rewards_oracle::SetCurrentRewardsWrapperArgsV1 {
                current_rewards: reward.reward.amount,
                oracle_index: reward.index,
            },
        }
        .data(),
    };
    Ok(ix)
}

pub async fn distribute_rewards_instruction<C: AsRef<DasClient> + GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
    kta: &helium_entity_manager::KeyToAssetV0,
    payer: Pubkey,
) -> Result<Instruction, Error> {
    let ld_account = lazy_distributor(client, subdao).await?;
    let (asset, asset_proof) = asset::for_kta_with_proof(client, kta).await?;
    let accounts = lazy_distributor::accounts::DistributeCompressionRewardsV0 {
        DistributeCompressionRewardsV0common:
            lazy_distributor::accounts::DistributeCompressionRewardsV0Common {
                payer,
                lazy_distributor: subdao.lazy_distributor(),
                associated_token_program: spl_associated_token_account::id(),
                rewards_mint: *subdao.mint(),
                rewards_escrow: ld_account.rewards_escrow,
                system_program: solana_sdk::system_program::ID,
                token_program: anchor_spl::token::ID,
                circuit_breaker_program: circuit_breaker::id(),
                owner: asset.ownership.owner,
                circuit_breaker: lazy_distributor_circuit_breaker(&ld_account),
                recipient: subdao.receipient_key_from_kta(kta),
                destination_account: subdao
                    .token()
                    .associated_token_adress(&asset.ownership.owner),
            },
        compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
        merkle_tree: asset.compression.tree,
    };

    let mut ix = Instruction {
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

    ix.accounts.extend_from_slice(&asset_proof.proof(Some(3))?);
    Ok(ix)
}

pub async fn claim<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
    amount: Option<u64>,
    encoded_entity_key: &entity_key::EncodedEntityKey,
    keypair: &Keypair,
) -> Result<Option<(Transaction, u64)>, Error> {
    let Some((mut txn, block_height)) = claim_transaction(
        client,
        subdao,
        amount,
        encoded_entity_key,
        &keypair.pubkey(),
    )
    .await?
    else {
        return Ok(None);
    };

    txn.try_sign(&[keypair], *txn.get_recent_blockhash())?;
    Ok(Some((txn, block_height)))
}

pub async fn claim_transaction<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
    amount: Option<u64>,
    encoded_entity_key: &entity_key::EncodedEntityKey,
    payer: &Pubkey,
) -> Result<Option<(Transaction, u64)>, Error> {
    let entity_key = encoded_entity_key.as_entity_key()?;
    let pending = pending(
        client,
        subdao,
        &[encoded_entity_key.entity_key.clone()],
        encoded_entity_key.encoding.into(),
    )
    .await?;

    if let Some(0) = amount {
        return Ok(None);
    }
    let Some(pending_reward) = pending.get(&encoded_entity_key.entity_key) else {
        return Ok(None);
    };
    let ld_account = lazy_distributor(client, subdao).await?;
    let max_claim = max_claim(client, subdao, &ld_account).await?;

    let mut current_reward = current(client, subdao, &entity_key).await?;

    let to_claim = amount
        .unwrap_or(pending_reward.reward.amount)
        .min(max_claim.amount);
    current_reward.reward.amount =
        current_reward.reward.amount - pending_reward.reward.amount + to_claim;

    let kta_key = Dao::Hnt.entity_key_to_kta_key(&entity_key);
    let kta = kta::for_entity_key(&entity_key).await?;

    let set_current_ix =
        set_current_rewards_instruction(subdao, kta_key, &kta, &current_reward).await?;
    let distribute_ix = distribute_rewards_instruction(client, subdao, &kta, *payer).await?;
    let mut ixs_accounts = set_current_ix.accounts.clone();
    ixs_accounts.extend_from_slice(&distribute_ix.accounts);

    let ixs = &[
        priority_fee::compute_budget_instruction(150_000),
        priority_fee::compute_price_instruction_for_accounts(client, &ixs_accounts).await?,
        set_current_ix,
        distribute_ix,
    ];

    let solana_client = AsRef::<SolanaRpcClient>::as_ref(client);
    let (latest_blockhash, latest_block_height) = solana_client
        .get_latest_blockhash_with_commitment(solana_client.commitment())
        .await?;
    let mut txn = Transaction::new_with_payer(ixs, Some(payer));
    txn.message.recent_blockhash = latest_blockhash;

    let signed_txn = oracle_sign(&current_reward.oracle.url, txn).await?;
    Ok(Some((signed_txn, latest_block_height)))
}

pub async fn current<E: AsEntityKey, C: AsRef<DasClient> + GetAnchorAccount>(
    client: &C,
    subdao: &SubDao,
    entity_key: &E,
) -> Result<OracleReward, Error> {
    let ld_account = lazy_distributor(client, subdao).await?;
    let oracle = ld_account
        .oracles
        .first()
        .ok_or_else(|| DecodeError::other("missing oracle in lazy distributor"))?;
    let asset = asset::for_entity_key(client, entity_key).await?;
    current_from_oracle(subdao, &oracle.url, &asset.id)
        .map_ok(|reward| OracleReward {
            reward,
            oracle: oracle.clone().into(),
            index: 0,
        })
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
    // collect entity keys to request all ktas at once
    let entity_keys: Vec<Vec<u8>> = entity_key_strings
        .iter()
        .map(|entity_key_string| entity_key::from_str(entity_key_string, entity_key_encoding))
        .try_collect()?;
    let ktas = kta::for_entity_keys(&entity_keys).await?;
    // Collect rewarded entities
    let (rewarded_entity_key_strings, rewarded_ktas, rewards): (
        Vec<String>,
        Vec<helium_entity_manager::KeyToAssetV0>,
        Vec<OracleReward>,
    ) = izip!(entity_key_strings, ktas)
        .map(|(entity_key_string, kta)| {
            for_entity_key(&bulk_rewards, entity_key_string)
                .map(|reward| (entity_key_string.to_owned(), kta, reward))
        })
        .flatten()
        .collect::<Vec<(String, helium_entity_manager::KeyToAssetV0, OracleReward)>>()
        .into_iter()
        .multiunzip();
    // Get all recipients for rewarded assets
    let recipients = recipient::for_ktas(client, subdao, &rewarded_ktas).await?;
    // And adjust the oracle reward by the already claimed rewards in the recipient if available
    let entity_key_rewards = izip!(rewarded_entity_key_strings, rewards, recipients)
        .map(|(entity_key_string, mut reward, maybe_recipient)| {
            if let Some(recipient) = maybe_recipient {
                reward.reward.amount = reward.reward.amount.saturating_sub(recipient.total_rewards);
            }
            (entity_key_string, reward)
        })
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
            |mut result, (index, oracle): (usize, lazy_distributor::OracleConfigV0)| async move {
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

async fn oracle_sign(oracle: &str, txn: Transaction) -> Result<Transaction, Error> {
    #[derive(Debug, Serialize, Deserialize)]
    struct Data {
        data: Vec<u8>,
    }
    #[derive(Debug, Serialize)]
    struct OracleSignRequest {
        transaction: Data,
    }
    #[derive(Debug, Deserialize)]
    struct OracleSignResponse {
        pub transaction: Data,
    }
    let client = reqwest::Client::new();
    let transaction = Data {
        data: bincode::serialize(&txn).map_err(EncodeError::from)?,
    };
    let response = client
        .post(oracle.to_string())
        .json(&OracleSignRequest { transaction })
        .send()
        .await?
        .json::<OracleSignResponse>()
        .await?;
    let signed_tx = bincode::deserialize(&response.transaction.data).map_err(DecodeError::from)?;
    Ok(signed_tx)
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

    pub async fn for_ktas<C: GetAnchorAccount>(
        client: &C,
        subdao: &SubDao,
        ktas: &[helium_entity_manager::KeyToAssetV0],
    ) -> Result<Vec<Option<lazy_distributor::RecipientV0>>, Error> {
        let recipient_keys: Vec<Pubkey> = ktas
            .iter()
            .map(|kta| subdao.receipient_key_from_kta(kta))
            .collect();
        client.anchor_accounts(&recipient_keys).await
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

        let ix = Instruction {
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
        Ok(ix)
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
