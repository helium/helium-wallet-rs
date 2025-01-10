use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    asset, circuit_breaker,
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    dao::Dao,
    entity_key::{self, AsEntityKey, KeySerialization},
    error::{DecodeError, EncodeError, Error},
    helium_entity_manager,
    keypair::{Keypair, Pubkey},
    kta, lazy_distributor, mk_transaction_with_blockhash, priority_fee,
    programs::SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
    rewards_oracle,
    solana_sdk::instruction::Instruction,
    token::{Token, TokenAmount},
    TransactionOpts, TransactionWithBlockhash,
};
use chrono::Utc;
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
    pub oracle: Oracle,
    pub index: u16,
    pub reward: TokenAmount,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum ClaimableToken {
    Iot,
    Mobile,
    Hnt,
}

impl From<ClaimableToken> for Token {
    fn from(value: ClaimableToken) -> Self {
        match value {
            ClaimableToken::Iot => Token::Iot,
            ClaimableToken::Mobile => Token::Mobile,
            ClaimableToken::Hnt => Token::Hnt,
        }
    }
}

impl ClaimableToken {
    pub fn mint(&self) -> &Pubkey {
        match self {
            Self::Iot => Token::Iot.mint(),
            Self::Mobile => Token::Mobile.mint(),
            Self::Hnt => Token::Hnt.mint(),
        }
    }
    pub fn lazy_distributor_key(&self) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[b"lazy_distributor", self.mint().as_ref()],
            &lazy_distributor::id(),
        );
        key
    }
    pub fn receipient_key_from_kta(&self, kta: &helium_entity_manager::KeyToAssetV0) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[
                b"recipient",
                self.lazy_distributor_key().as_ref(),
                kta.asset.as_ref(),
            ],
            &lazy_distributor::id(),
        );
        key
    }
}

pub async fn lazy_distributor<C: GetAnchorAccount>(
    client: &C,
    token: ClaimableToken,
) -> Result<lazy_distributor::LazyDistributorV0, Error> {
    client
        .anchor_account::<lazy_distributor::LazyDistributorV0>(&token.lazy_distributor_key())
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

fn time_decay_previous_value(
    config: &circuit_breaker::WindowedCircuitBreakerConfigV0,
    window: &circuit_breaker::WindowV0,
    unix_timestamp: i64,
) -> Option<u64> {
    let time_elapsed = unix_timestamp.checked_sub(window.last_unix_timestamp)?;
    u64::try_from(
        u128::from(window.last_aggregated_value)
            .checked_mul(
                // (window_size_seconds - min(window_size_seconds, time_elapsed)) / window_size_seconds
                // = (1 -  min((time_elapsed / window_size_seconds), 1))
                u128::from(config.window_size_seconds.checked_sub(std::cmp::min(
                    u64::try_from(time_elapsed).ok()?,
                    config.window_size_seconds,
                ))?),
            )?
            .checked_div(u128::from(config.window_size_seconds))?,
    )
    .ok()
}

pub async fn max_claim<C: GetAnchorAccount>(
    client: &C,
    token: ClaimableToken,
) -> Result<TokenAmount, Error> {
    let ld_account = lazy_distributor(client, token).await?;
    let circuit_breaker_account: circuit_breaker::AccountWindowedCircuitBreakerV0 = client
        .anchor_account(&lazy_distributor_circuit_breaker(&ld_account))
        .await?;
    let threshold = match circuit_breaker_account.config {
        circuit_breaker::WindowedCircuitBreakerConfigV0 {
            threshold_type: circuit_breaker::ThresholdType::Absolute,
            threshold,
            ..
        } => threshold,
        _ => return Err(DecodeError::other("percent max claim threshold not supported").into()),
    };
    let remaining = time_decay_previous_value(
        &circuit_breaker_account.config,
        &circuit_breaker_account.last_window,
        Utc::now().timestamp(),
    )
    .ok_or_else(|| DecodeError::other("failed to calculate decayed rewards"))?;
    Ok(Token::from(token).amount(threshold - remaining))
}

async fn set_current_rewards_instruction(
    token: ClaimableToken,
    kta_key: Pubkey,
    kta: &helium_entity_manager::KeyToAssetV0,
    reward: &OracleReward,
) -> Result<Instruction, Error> {
    let accounts = rewards_oracle::accounts::SetCurrentRewardsWrapperV1 {
        oracle: reward.oracle.key,
        lazy_distributor: token.lazy_distributor_key(),
        recipient: token.receipient_key_from_kta(kta),
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
    token: ClaimableToken,
    kta: &helium_entity_manager::KeyToAssetV0,
    asset: &asset::Asset,
    asset_proof: &asset::AssetProof,
    payer: Pubkey,
) -> Result<Instruction, Error> {
    let ld_account = lazy_distributor(client, token).await?;
    let accounts = lazy_distributor::accounts::DistributeCompressionRewardsV0 {
        DistributeCompressionRewardsV0common:
            lazy_distributor::accounts::DistributeCompressionRewardsV0Common {
                payer,
                lazy_distributor: token.lazy_distributor_key(),
                associated_token_program: spl_associated_token_account::id(),
                rewards_mint: *token.mint(),
                rewards_escrow: ld_account.rewards_escrow,
                system_program: solana_sdk::system_program::ID,
                token_program: anchor_spl::token::ID,
                circuit_breaker_program: circuit_breaker::id(),
                owner: asset.ownership.owner,
                circuit_breaker: lazy_distributor_circuit_breaker(&ld_account),
                recipient: token.receipient_key_from_kta(kta),
                destination_account: Token::from(token)
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
    token: ClaimableToken,
    amount: Option<u64>,
    encoded_entity_key: &entity_key::EncodedEntityKey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<Option<TransactionWithBlockhash>, Error> {
    let Some(mut txn) = claim_transaction(
        client,
        token,
        amount,
        encoded_entity_key,
        &keypair.pubkey(),
        opts,
    )
    .await?
    else {
        return Ok(None);
    };

    txn.try_sign(&[keypair])?;
    Ok(Some(txn))
}

pub async fn claim_transaction<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    token: ClaimableToken,
    amount: Option<u64>,
    encoded_entity_key: &entity_key::EncodedEntityKey,
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<Option<TransactionWithBlockhash>, Error> {
    let entity_key_string = encoded_entity_key.to_string();
    let pending = pending(
        client,
        token,
        &[encoded_entity_key.to_string()],
        encoded_entity_key.encoding.into(),
    )
    .await?;

    if let Some(0) = amount {
        return Ok(None);
    }
    let Some(pending_reward) = pending.get(&entity_key_string) else {
        return Ok(None);
    };
    let max_claim = max_claim(client, token).await?;

    let mut lifetime_rewards = lifetime(client, token, &[entity_key_string.clone()])
        .and_then(|mut lifetime_map| async move {
            lifetime_map
                .remove(&entity_key_string)
                .and_then(|mut rewards| rewards.pop())
                .ok_or(Error::account_not_found())
        })
        .await?;

    let to_claim = amount
        .unwrap_or(pending_reward.reward.amount)
        .min(max_claim.amount);
    lifetime_rewards.reward.amount =
        lifetime_rewards.reward.amount - pending_reward.reward.amount + to_claim;

    let entity_key = encoded_entity_key.as_entity_key()?;
    let kta_key = Dao::Hnt.entity_key_to_kta_key(&entity_key);
    let kta = kta::for_entity_key(&entity_key).await?;
    let (asset, asset_proof) = asset::for_kta_with_proof(client, &kta).await?;

    let (init_ix, init_budget) = if recipient::for_kta(client, token, &kta).await?.is_none() {
        let ix = recipient::init_instruction(token, &kta, &asset, &asset_proof, payer).await?;
        (Some(ix), recipient::INIT_INSTRUCTION_BUDGET)
    } else {
        (None, 1)
    };
    let set_current_ix =
        set_current_rewards_instruction(token, kta_key, &kta, &lifetime_rewards).await?;
    let distribute_ix =
        distribute_rewards_instruction(client, token, &kta, &asset, &asset_proof, *payer).await?;
    let mut ixs_accounts = vec![];
    if let Some(ix) = &init_ix {
        ixs_accounts.extend_from_slice(&ix.accounts);
    }
    ixs_accounts.extend_from_slice(&set_current_ix.accounts);
    ixs_accounts.extend_from_slice(&distribute_ix.accounts);

    let mut ixs = vec![
        priority_fee::compute_budget_instruction(init_budget + 200_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &ixs_accounts,
            opts.min_priority_fee,
        )
        .await?,
    ];
    if let Some(ix) = init_ix {
        ixs.push(ix);
    }
    ixs.extend_from_slice(&[set_current_ix, distribute_ix]);

    let txn = mk_transaction_with_blockhash(client, &ixs, payer).await?;
    let signed_txn = oracle_sign(&lifetime_rewards.oracle.url, txn).await?;
    Ok(Some(signed_txn))
}

pub async fn pending<C: GetAnchorAccount>(
    client: &C,
    token: ClaimableToken,
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

    let bulk_rewards = lifetime(client, token, entity_key_strings).await?;
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
    let recipients = recipient::for_ktas(client, token, &rewarded_ktas).await?;
    // And adjust the oracle reward by the already claimed rewards in the recipient if available
    let entity_key_rewards = izip!(rewarded_entity_key_strings, rewards, recipients)
        .filter_map(|(entity_key_string, mut reward, maybe_recipient)| {
            if let Some(recipient) = maybe_recipient {
                reward.reward.amount = reward.reward.amount.saturating_sub(recipient.total_rewards);
            }
            // Filter out 0 rewards
            if reward.reward.amount == 0 {
                None
            } else {
                Some((entity_key_string, reward))
            }
        })
        .collect();
    Ok(entity_key_rewards)
}

pub async fn lifetime<C: GetAnchorAccount>(
    client: &C,
    token: ClaimableToken,
    entity_key_strings: &[String],
) -> Result<HashMap<String, Vec<OracleReward>>, Error> {
    let ld_account = lazy_distributor(client, token).await?;
    stream::iter(ld_account.oracles)
        .enumerate()
        .map(Ok)
        .try_fold(
            HashMap::new(),
            |mut result, (index, oracle): (usize, lazy_distributor::OracleConfigV0)| async move {
                let bulk_rewards = bulk_from_oracle(token, &oracle.url, entity_key_strings).await?;
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

async fn oracle_sign(
    oracle: &str,
    txn: TransactionWithBlockhash,
) -> Result<TransactionWithBlockhash, Error> {
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
        data: bincode::serialize(&txn.inner).map_err(EncodeError::from)?,
    };
    let response = client
        .post(oracle.to_string())
        .json(&OracleSignRequest { transaction })
        .send()
        .await?
        .json::<OracleSignResponse>()
        .await?;
    let signed_tx = bincode::deserialize(&response.transaction.data).map_err(DecodeError::from)?;
    Ok(txn.with_signed_transaction(signed_tx))
}

async fn bulk_from_oracle(
    token: ClaimableToken,
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
            value_to_token_amount(token, value).map(|amount| (entity_key_string, amount))
        })
        .try_collect()
}

pub mod recipient {
    use super::*;

    pub async fn for_kta<C: GetAnchorAccount>(
        client: &C,
        token: ClaimableToken,
        kta: &helium_entity_manager::KeyToAssetV0,
    ) -> Result<Option<lazy_distributor::RecipientV0>, Error> {
        let recipient_key = token.receipient_key_from_kta(kta);
        Ok(client.anchor_account(&recipient_key).await.ok())
    }

    pub async fn for_ktas<C: GetAnchorAccount>(
        client: &C,
        token: ClaimableToken,
        ktas: &[helium_entity_manager::KeyToAssetV0],
    ) -> Result<Vec<Option<lazy_distributor::RecipientV0>>, Error> {
        let recipient_keys: Vec<Pubkey> = ktas
            .iter()
            .map(|kta| token.receipient_key_from_kta(kta))
            .collect();
        client.anchor_accounts(&recipient_keys).await
    }

    pub async fn init_instruction(
        token: ClaimableToken,
        kta: &helium_entity_manager::KeyToAssetV0,
        asset: &asset::Asset,
        asset_proof: &asset::AssetProof,
        payer: &Pubkey,
    ) -> Result<Instruction, Error> {
        fn mk_accounts(
            payer: Pubkey,
            owner: Pubkey,
            tree: Pubkey,
            token: ClaimableToken,
            kta: &helium_entity_manager::KeyToAssetV0,
        ) -> impl ToAccountMetas {
            lazy_distributor::accounts::InitializeCompressionRecipientV0 {
                payer,
                lazy_distributor: token.lazy_distributor_key(),
                recipient: token.receipient_key_from_kta(kta),
                merkle_tree: tree,
                owner,
                delegate: owner,
                compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                system_program: solana_sdk::system_program::id(),
            }
        }

        let mut accounts = mk_accounts(
            *payer,
            asset.ownership.owner,
            asset.compression.tree,
            token,
            kta,
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

    pub const INIT_INSTRUCTION_BUDGET: u32 = 150_000;

    pub async fn init<E: AsEntityKey, C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
        client: &C,
        token: ClaimableToken,
        entity_key: &E,
        keypair: &Keypair,
        opts: &TransactionOpts,
    ) -> Result<TransactionWithBlockhash, Error> {
        let kta = kta::for_entity_key(entity_key).await?;
        let (asset, asset_proof) = asset::for_kta_with_proof(client, &kta).await?;

        let ix = init_instruction(token, &kta, &asset, &asset_proof, &keypair.pubkey()).await?;
        let ixs = &[
            priority_fee::compute_budget_instruction(INIT_INSTRUCTION_BUDGET),
            priority_fee::compute_price_instruction_for_accounts(
                client,
                &ix.accounts,
                opts.min_priority_fee,
            )
            .await?,
            ix,
        ];
        let mut txn = mk_transaction_with_blockhash(client, ixs, &keypair.pubkey()).await?;
        txn.try_sign(&[keypair])?;
        Ok(txn)
    }
}

fn value_to_token_amount(
    token: ClaimableToken,
    value: serde_json::Value,
) -> Result<TokenAmount, Error> {
    let value = match value {
        serde_json::Value::String(s) => s
            .parse::<u64>()
            .map_err(|_| DecodeError::other(format!("invalid reward value {s}")))?,
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| DecodeError::other(format!("invalid reward value {n}")))?,
        _ => return Err(DecodeError::other(format!("invalid reward value {value}")).into()),
    };

    Ok(TokenAmount::from_u64(token.into(), value))
}
