use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    asset, b64, circuit_breaker,
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    dao::Dao,
    ed25519_instruction,
    entity_key::{self, AsEntityKey, EncodedEntityKey},
    error::{DecodeError, Error},
    helium_entity_manager,
    keypair::{Keypair, Pubkey},
    kta, lazy_distributor,
    message::{self, mk_message},
    priority_fee, rewards_oracle,
    solana_sdk::{instruction::Instruction, signer::Signer, sysvar},
    spl_account_compression,
    token::{Token, TokenAmount},
    transaction::{mk_transaction, VersionedTransaction},
    TransactionOpts,
};
use chrono::Utc;
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use itertools::{izip, Itertools};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Display};

#[derive(Debug, Serialize, Clone)]
pub struct Oracle {
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub key: Pubkey,
    pub url: String,
}

impl From<lazy_distributor::types::OracleConfigV0> for Oracle {
    fn from(value: lazy_distributor::types::OracleConfigV0) -> Self {
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

#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum ClaimableToken {
    Iot,
    Mobile,
    #[default]
    Hnt,
}

impl Display for ClaimableToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Self::Iot => "iot",
            Self::Mobile => "mobile",
            Self::Hnt => "hnt",
        };
        f.write_str(str)
    }
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
            &lazy_distributor::ID,
        );
        key
    }
    pub fn recipient_key_from_kta(
        &self,
        kta: &helium_entity_manager::accounts::KeyToAssetV0,
    ) -> Pubkey {
        let (key, _) = Pubkey::find_program_address(
            &[
                b"recipient",
                self.lazy_distributor_key().as_ref(),
                kta.asset.as_ref(),
            ],
            &lazy_distributor::ID,
        );
        key
    }
}

pub async fn lazy_distributor<C: GetAnchorAccount>(
    client: &C,
    token: ClaimableToken,
) -> Result<lazy_distributor::accounts::LazyDistributorV0, Error> {
    client
        .anchor_account::<lazy_distributor::accounts::LazyDistributorV0>(
            &token.lazy_distributor_key(),
        )
        .await
}

pub fn lazy_distributor_circuit_breaker(
    ld_account: &lazy_distributor::accounts::LazyDistributorV0,
) -> Pubkey {
    let (circuit_breaker, _) = Pubkey::find_program_address(
        &[
            b"account_windowed_breaker",
            ld_account.rewards_escrow.as_ref(),
        ],
        &circuit_breaker::ID,
    );
    circuit_breaker
}

fn time_decay_previous_value(
    config: &circuit_breaker::types::WindowedCircuitBreakerConfigV0,
    window: &circuit_breaker::types::WindowV0,
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
    let circuit_breaker_account: circuit_breaker::accounts::AccountWindowedCircuitBreakerV0 =
        client
            .anchor_account(&lazy_distributor_circuit_breaker(&ld_account))
            .await?;
    let threshold = match circuit_breaker_account.config {
        circuit_breaker::types::WindowedCircuitBreakerConfigV0 {
            threshold_type: circuit_breaker::types::ThresholdType::Absolute,
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

fn set_current_rewards_instruction(
    token: ClaimableToken,
    kta_key: &Pubkey,
    kta: &helium_entity_manager::accounts::KeyToAssetV0,
    reward: &OracleReward,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    let accounts = rewards_oracle::client::accounts::SetCurrentRewardsWrapperV2 {
        lazy_distributor: token.lazy_distributor_key(),
        recipient: token.recipient_key_from_kta(kta),
        payer: payer.to_owned(),
        lazy_distributor_program: lazy_distributor::ID,
        system_program: solana_sdk::system_program::ID,
        key_to_asset: *kta_key,
        oracle_signer: Dao::oracle_signer_key(),
        sysvar_instructions: sysvar::instructions::ID,
    }
    .to_account_metas(None);

    let ix = Instruction {
        program_id: rewards_oracle::ID,
        accounts,
        data: rewards_oracle::client::args::SetCurrentRewardsWrapperV2 {
            args: rewards_oracle::types::SetCurrentRewardsWrapperArgsV1 {
                current_rewards: reward.reward.amount,
                oracle_index: reward.index,
            },
        }
        .data(),
    };
    Ok(ix)
}

pub fn distribute_rewards_instruction_for_destination(
    token: ClaimableToken,
    ld_account: &lazy_distributor::accounts::LazyDistributorV0,
    kta: &helium_entity_manager::accounts::KeyToAssetV0,
    asset: &asset::Asset,
    destination_account: &Pubkey,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    let accounts = lazy_distributor::client::accounts::DistributeCustomDestinationV0 {
        common_1: lazy_distributor::client::accounts::Common {
            payer: *payer,
            lazy_distributor: token.lazy_distributor_key(),
            associated_token_program: spl_associated_token_account::ID,
            rewards_mint: *token.mint(),
            rewards_escrow: ld_account.rewards_escrow,
            system_program: solana_sdk::system_program::ID,
            token_program: anchor_spl::token::ID,
            circuit_breaker_program: circuit_breaker::ID,
            owner: asset.ownership.owner,
            circuit_breaker: lazy_distributor_circuit_breaker(ld_account),
            recipient: token.recipient_key_from_kta(kta),
            destination_account: Token::from(token).associated_token_adress(destination_account),
        },
    }
    .to_account_metas(None);

    let ix = Instruction {
        accounts,
        program_id: lazy_distributor::ID,
        data: lazy_distributor::client::args::DistributeCustomDestinationV0 {}.data(),
    };

    Ok(ix)
}

pub fn distribute_rewards_instruction_for_owner(
    token: ClaimableToken,
    ld_account: &lazy_distributor::accounts::LazyDistributorV0,
    kta: &helium_entity_manager::accounts::KeyToAssetV0,
    asset_with_proof: &(asset::Asset, asset::AssetProof),
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    use lazy_distributor::client::accounts::{Common, DistributeCompressionRewardsV0};
    let (asset, asset_proof) = asset_with_proof;
    let mut accounts = DistributeCompressionRewardsV0 {
        common: Common {
            payer: *payer,
            lazy_distributor: token.lazy_distributor_key(),
            associated_token_program: spl_associated_token_account::ID,
            rewards_mint: *token.mint(),
            rewards_escrow: ld_account.rewards_escrow,
            system_program: solana_sdk::system_program::ID,
            token_program: anchor_spl::token::ID,
            circuit_breaker_program: circuit_breaker::ID,
            owner: asset.ownership.owner,
            circuit_breaker: lazy_distributor_circuit_breaker(ld_account),
            recipient: token.recipient_key_from_kta(kta),
            destination_account: Token::from(token).associated_token_adress(&asset.ownership.owner),
        },
        compression_program: spl_account_compression::ID,
        merkle_tree: asset.compression.tree,
    }
    .to_account_metas(None);
    accounts.append(&mut asset_proof.proof(Some(3))?);

    let ix = Instruction {
        accounts,
        program_id: lazy_distributor::ID,
        data: lazy_distributor::client::args::DistributeCompressionRewardsV0 {
            args: lazy_distributor::types::DistributeCompressionRewardsArgsV0 {
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

pub async fn claim<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    token: ClaimableToken,
    amount: Option<u64>,
    encoded_entity_key: &entity_key::EncodedEntityKey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<Option<(VersionedTransaction, u64)>, Error> {
    let ticket = ClaimTicket::new(encoded_entity_key.to_owned(), amount)?;
    let Some(Some(instructions)) = claim_instructions(client, token, &[ticket], &keypair.pubkey())
        .await?
        .pop()
    else {
        return Ok(None);
    };

    let (msg, block_height) = mk_message(
        client,
        &instructions,
        &opts.lut_addresses,
        &keypair.pubkey(),
    )
    .await?;
    let txn = mk_transaction(msg, &[keypair])?;
    Ok(Some((txn, block_height)))
}

pub struct ClaimCommon<'a> {
    pub token: ClaimableToken,
    pub ld_account: &'a lazy_distributor::accounts::LazyDistributorV0,
    pub kta_key: &'a Pubkey,
    pub kta: &'a helium_entity_manager::accounts::KeyToAssetV0,
    pub asset: asset::Asset,
    pub payer: &'a Pubkey,
    pub rewards: &'a OracleReward,
    pub oracle_ixn: Instruction,
}

pub fn claim_instructions_for_destination(
    common: ClaimCommon,
    destination: &Pubkey,
) -> Result<Vec<Instruction>, Error> {
    let set_current_ix = set_current_rewards_instruction(
        common.token,
        common.kta_key,
        common.kta,
        common.rewards,
        common.payer,
    )?;
    let distribute_ix = distribute_rewards_instruction_for_destination(
        common.token,
        common.ld_account,
        common.kta,
        &common.asset,
        destination,
        common.payer,
    )?;
    let ixs = [common.oracle_ixn, set_current_ix, distribute_ix].to_vec();
    Ok(ixs)
}

pub fn claim_instructions_for_owner(
    common: ClaimCommon,
    asset_proof: asset::AssetProof,
    init_recipient: bool,
) -> Result<Vec<Instruction>, Error> {
    let asset_with_proof = &(common.asset, asset_proof);
    let init_ix = init_recipient
        .then(|| {
            recipient::init_instruction(common.token, common.kta, asset_with_proof, common.payer)
        })
        .transpose()?;
    let set_current_ix = set_current_rewards_instruction(
        common.token,
        common.kta_key,
        common.kta,
        common.rewards,
        common.payer,
    )?;
    let distribute_ix = distribute_rewards_instruction_for_owner(
        common.token,
        common.ld_account,
        common.kta,
        asset_with_proof,
        common.payer,
    )?;
    let ixs = [
        init_ix,
        Some(common.oracle_ixn),
        Some(set_current_ix),
        Some(distribute_ix),
    ]
    .into_iter()
    .flatten()
    .collect_vec();
    Ok(ixs)
}

pub struct ClaimTicket {
    pub amount: Option<u64>,
    pub encoded_entity_key: EncodedEntityKey,

    pub kta_key: Pubkey,
    pub entity_key: Vec<u8>,
}

impl ClaimTicket {
    pub fn new(
        encoded_entity_key: entity_key::EncodedEntityKey,
        amount: Option<u64>,
    ) -> Result<Self, Error> {
        let entity_key = encoded_entity_key.as_entity_key()?;
        let kta_key = Dao::Hnt.entity_key_to_kta_key(&entity_key);
        Ok(Self {
            amount,
            encoded_entity_key,
            entity_key,
            kta_key,
        })
    }

    pub fn key_str(&self) -> &str {
        &self.encoded_entity_key.entity_key
    }

    pub fn key(&self) -> String {
        self.encoded_entity_key.to_string()
    }
}

impl AsRef<EncodedEntityKey> for ClaimTicket {
    fn as_ref(&self) -> &EncodedEntityKey {
        &self.encoded_entity_key
    }
}

impl AsEntityKey for ClaimTicket {
    fn as_entity_key(&self) -> Vec<u8> {
        self.entity_key.clone()
    }
}

pub async fn claim_instructions<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    token: ClaimableToken,
    tickets: &[ClaimTicket],
    payer: &Pubkey,
) -> Result<Vec<Option<Vec<Instruction>>>, Error> {
    let mut lifetime_rewards = lifetime(client, token, tickets).await?;
    let pending_map = pending(client, token, Some(&lifetime_rewards), tickets).await?;
    let mut asset_map: HashMap<String, asset::Asset> = izip!(
        tickets.iter().map(|t| t.encoded_entity_key.to_string()),
        asset::for_entity_keys(client, tickets).await?
    )
    .collect();
    let recipient_map: HashMap<String, lazy_distributor::accounts::RecipientV0> = tickets
        .iter()
        .zip(recipient::for_entity_keys(client, token, tickets).await?)
        .filter_map(|(ticket, maybe_recipient)| {
            maybe_recipient.map(|recipient| (ticket.encoded_entity_key.to_string(), recipient))
        })
        .collect();
    let mut asset_proof_map: HashMap<String, asset::AssetProof> = {
        let (entity_key_strings, needed_proof_keys): (Vec<String>, Vec<Pubkey>) = asset_map
            .iter()
            .filter_map(|(entity_key_str, asset)| {
                if let Some(recipient) = recipient_map.get(entity_key_str) {
                    (recipient.destination == Pubkey::default())
                        .then_some((entity_key_str.clone(), asset.id))
                } else {
                    Some((entity_key_str.clone(), asset.id))
                }
            })
            .unzip();
        izip!(
            entity_key_strings,
            asset::proof::get_many(client, &needed_proof_keys).await?
        )
        .collect()
    };

    let max_claim = max_claim(client, token).await?;
    let to_claim_rewards: HashMap<String, OracleReward> = tickets
        .iter()
        .filter_map(|ticket| {
            let mut oracle_reward = lifetime_rewards.remove(ticket.key_str())?.pop()?;
            let pending_reward = pending_map.get(ticket.key_str())?;
            let max_pending = pending_reward.reward.amount;
            // ensyre that the requested claim amount is the lower of max_claim and the pending amount
            let to_claim = ticket
                .amount
                .unwrap_or(max_pending)
                .min(max_claim.amount)
                .min(max_pending);
            if to_claim == 0 {
                return None;
            }
            oracle_reward.reward.amount = oracle_reward.reward.amount - max_pending + to_claim;
            Some((ticket.key(), oracle_reward))
        })
        .collect();
    let mut oracle_ixns: HashMap<String, Instruction> = {
        // Do a shuffle to get rewards for tickets grouped bu oracle urls, while excluding tickets with no
        // rewards
        let oracle_chunks = tickets
            .iter()
            .filter_map(|ticket| {
                to_claim_rewards
                    .get(ticket.key_str())
                    .map(|reward| (ticket, reward.oracle.url.as_str()))
            })
            .chunk_by(|(_ticket, oracle_url)| *oracle_url);
        // Then stream over these chunks getting sign instrucitons for each
        stream::iter(oracle_chunks.into_iter().map(|(url, chunk)| {
            // For ecah chunk collect the entity string and the kta keys to fetch
            let (entity_key_strings, kta_keys): (Vec<String>, Vec<Pubkey>) = chunk
                .into_iter()
                .map(|(ticket, _)| (ticket.encoded_entity_key.to_string(), ticket.kta_key))
                .unzip();
            Ok((url, entity_key_strings, kta_keys))
        }))
        .map_ok(|(url, entity_key_strings, kta_keys)| async move {
            // Then fetch the instructions for the kta_keys and stream them back out tied to the entity_keys
            oracle_sign_instructions(url, &kta_keys)
                .map_ok(|ixns| {
                    stream::iter(
                        izip!(entity_key_strings, ixns).map(Ok::<(String, Instruction), Error>),
                    )
                })
                .await
        })
        .try_buffered(5)
        .try_flatten()
        .try_collect()
        .await?
    };

    let ld_account = lazy_distributor(client, token).await?;
    tickets
        .iter()
        .map(|ticket| {
            let Some(rewards) = to_claim_rewards.get(ticket.key_str()) else {
                return Ok(None);
            };
            // Should be safe because all ktas were fetched as part of the asset fetch
            let kta = kta::get_cached(&ticket.kta_key).expect("kta for {ticket_key}");
            // Safe to unwrap since asset::get_many will fail for any not found asset
            let asset = asset_map
                .remove(ticket.key_str())
                .expect("asset for {ticket_key}");
            // Safe to unwrap since an oracle ixn must exist for every reward
            let oracle_ixn = oracle_ixns
                .remove(ticket.key_str())
                .expect("oracle instruction for {ticket_key}");
            let claim_common = ClaimCommon {
                token,
                ld_account: &ld_account,
                asset,
                kta_key: &ticket.kta_key,
                kta: &kta,
                oracle_ixn,
                rewards,
                payer,
            };
            let claim_ix = if let Some(recipient) = recipient_map.get(ticket.key_str()) {
                if recipient.destination != Pubkey::default() {
                    claim_instructions_for_destination(claim_common, &recipient.destination)
                } else {
                    let asset_proof = asset_proof_map
                        .remove(ticket.key_str())
                        .expect("asset proof for default recipient");
                    claim_instructions_for_owner(claim_common, asset_proof, false)
                }
            } else {
                let asset_proof = asset_proof_map
                    .remove(ticket.key_str())
                    .expect("asset proof for absent recipient");
                claim_instructions_for_owner(claim_common, asset_proof, true)
            };
            claim_ix.map(Some)
        })
        .try_collect()
}

pub async fn pending_amounts<C: GetAnchorAccount, E: AsRef<EncodedEntityKey>>(
    client: &C,
    token: ClaimableToken,
    lifetime_rewards: Option<&HashMap<String, Vec<OracleReward>>>,
    encoded_entity_keys: &[E],
) -> Result<HashMap<String, TokenAmount>, Error> {
    pending(client, token, lifetime_rewards, encoded_entity_keys)
        .map_ok(|pending| {
            pending
                .into_iter()
                .map(|(key, oracle_reward)| (key, oracle_reward.reward))
                .collect()
        })
        .await
}

pub async fn pending<C: GetAnchorAccount, E: AsRef<EncodedEntityKey>>(
    client: &C,
    token: ClaimableToken,
    lifetime_rewards: Option<&HashMap<String, Vec<OracleReward>>>,
    encoded_entity_keys: &[E],
) -> Result<HashMap<String, OracleReward>, Error> {
    fn for_entity_key(
        lifetime_rewards: &HashMap<String, Vec<OracleReward>>,
        entity_key_string: &str,
    ) -> Option<OracleReward> {
        let oracle_rewards = lifetime_rewards.get(entity_key_string)?;
        let mut sorted_oracle_rewards = oracle_rewards.clone();
        sorted_oracle_rewards.sort_unstable_by_key(|oracle_reward| oracle_reward.reward.amount);
        Some(sorted_oracle_rewards.remove(sorted_oracle_rewards.len() / 2))
    }

    let lifetime_rewards = if let Some(lifetime_rewards) = lifetime_rewards {
        lifetime_rewards
    } else {
        &lifetime(client, token, encoded_entity_keys).await?
    };
    let (entity_key_strings, entity_keys): (Vec<String>, Vec<Vec<u8>>) = {
        let tuple_vec: Vec<(String, Vec<u8>)> = encoded_entity_keys
            .iter()
            .map(|encoded| {
                let encoded_ref = encoded.as_ref();
                encoded_ref
                    .as_entity_key()
                    .map(|entity_key| (encoded_ref.to_string(), entity_key))
            })
            .try_collect()?;
        tuple_vec.into_iter().unzip()
    };
    // collect entity keys to request all ktas at once
    let ktas = kta::for_entity_keys(&entity_keys).await?;
    // Collect rewarded entities
    let (rewarded_entity_key_strings, rewarded_ktas, rewards): (
        Vec<String>,
        Vec<helium_entity_manager::accounts::KeyToAssetV0>,
        Vec<OracleReward>,
    ) = izip!(entity_key_strings, ktas)
        .map(|(entity_key_string, kta)| {
            for_entity_key(lifetime_rewards, &entity_key_string)
                .map(|reward| (entity_key_string.to_owned(), kta, reward))
        })
        .flatten()
        .collect::<Vec<(
            String,
            helium_entity_manager::accounts::KeyToAssetV0,
            OracleReward,
        )>>()
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

pub async fn lifetime<C: GetAnchorAccount, E: AsRef<EncodedEntityKey>>(
    client: &C,
    token: ClaimableToken,
    encoded_entity_keys: &[E],
) -> Result<HashMap<String, Vec<OracleReward>>, Error> {
    let ld_account = lazy_distributor(client, token).await?;
    for key in encoded_entity_keys {
        println!("{}", key.as_ref().entity_key.as_str());
    }
    stream::iter(ld_account.oracles)
        .enumerate()
        .map(Ok)
        .try_fold(
            HashMap::new(),
            |mut result, (index, oracle): (usize, lazy_distributor::types::OracleConfigV0)| async move {
                let bulk_rewards =
                    bulk_from_oracle(token, &oracle.url, encoded_entity_keys).await?;
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

async fn oracle_sign_instructions(
    oracle: &str,
    kta_keys: &[Pubkey],
) -> Result<Vec<Instruction>, Error> {
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OracleSignRequest {
        key_to_asset_keys: Vec<String>,
    }
    #[derive(Debug, Deserialize)]
    struct OracleSignResponse {
        pub messages: Vec<OracleSignMessage>,
        #[serde(with = "crate::keypair::serde_pubkey")]
        pub oracle: Pubkey,
    }
    #[derive(Debug, Deserialize)]
    struct OracleSignMessage {
        pub serialized: String,
        pub signature: String,
    }
    let client = reqwest::Client::new();
    let req = OracleSignRequest {
        key_to_asset_keys: kta_keys.iter().map(ToString::to_string).collect(),
    };
    let response = client
        .post(format!("{oracle}/v1/sign"))
        .json(&req)
        .send()
        .await?
        .json::<OracleSignResponse>()
        .await?;
    response
        .messages
        .into_iter()
        .map(|msg| {
            let message = b64::decode(msg.serialized)?;
            let signature = b64::decode(msg.signature)?;
            let ixn = ed25519_instruction::mk_instruction(&response.oracle, &message, &signature);
            Ok(ixn)
        })
        .try_collect()
}

async fn bulk_from_oracle<E: AsRef<EncodedEntityKey>>(
    token: ClaimableToken,
    oracle: &str,
    encoded_entity_keys: &[E],
) -> Result<HashMap<String, TokenAmount>, Error> {
    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct OracleBulkRewardRequest<'a> {
        entity_keys: Vec<&'a str>,
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
            entity_keys: encoded_entity_keys
                .iter()
                .map(|v| v.as_ref().entity_key.as_str())
                .collect(),
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
        kta: &helium_entity_manager::accounts::KeyToAssetV0,
    ) -> Result<Option<lazy_distributor::accounts::RecipientV0>, Error> {
        let recipient_key = token.recipient_key_from_kta(kta);
        Ok(client.anchor_account(&recipient_key).await.ok())
    }

    pub async fn for_entity_key<C: GetAnchorAccount, E: AsEntityKey>(
        client: &C,
        token: ClaimableToken,
        entity_key: &E,
    ) -> Result<Option<lazy_distributor::accounts::RecipientV0>, Error> {
        let kta = kta::for_entity_key(entity_key).await?;
        for_kta(client, token, &kta).await
    }

    pub async fn for_ktas<C: GetAnchorAccount>(
        client: &C,
        token: ClaimableToken,
        ktas: &[helium_entity_manager::accounts::KeyToAssetV0],
    ) -> Result<Vec<Option<lazy_distributor::accounts::RecipientV0>>, Error> {
        let recipient_keys: Vec<Pubkey> = ktas
            .iter()
            .map(|kta| token.recipient_key_from_kta(kta))
            .collect();
        client.anchor_accounts(&recipient_keys).await
    }

    pub async fn for_entity_keys<C: GetAnchorAccount, E: AsEntityKey>(
        client: &C,
        token: ClaimableToken,
        entity_keys: &[E],
    ) -> Result<Vec<Option<lazy_distributor::accounts::RecipientV0>>, Error> {
        let ktas = kta::for_entity_keys(entity_keys).await?;
        for_ktas(client, token, &ktas).await
    }

    pub fn init_instruction(
        token: ClaimableToken,
        kta: &helium_entity_manager::accounts::KeyToAssetV0,
        asset_with_proof: &(asset::Asset, asset::AssetProof),
        payer: &Pubkey,
    ) -> Result<Instruction, Error> {
        fn mk_accounts(
            payer: Pubkey,
            owner: Pubkey,
            tree: Pubkey,
            token: ClaimableToken,
            kta: &helium_entity_manager::accounts::KeyToAssetV0,
        ) -> impl ToAccountMetas {
            lazy_distributor::client::accounts::InitializeCompressionRecipientV0 {
                payer,
                lazy_distributor: token.lazy_distributor_key(),
                recipient: token.recipient_key_from_kta(kta),
                merkle_tree: tree,
                owner,
                delegate: owner,
                compression_program: spl_account_compression::ID,
                system_program: solana_sdk::system_program::ID,
            }
        }
        let (asset, asset_proof) = asset_with_proof;
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
            program_id: lazy_distributor::ID,
            accounts: accounts.to_account_metas(None),
            data: lazy_distributor::client::args::InitializeCompressionRecipientV0 {
                args: lazy_distributor::types::InitializeCompressionRecipientArgsV0 {
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

    pub async fn init_message<E: AsEntityKey, C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
        client: &C,
        token: ClaimableToken,
        entity_key: &E,
        payer: &Pubkey,
        opts: &TransactionOpts,
    ) -> Result<(message::VersionedMessage, u64), Error> {
        let kta = kta::for_entity_key(entity_key).await?;
        let asset_with_proof = asset::for_kta_with_proof(client, &kta).await?;

        let ix = init_instruction(token, &kta, &asset_with_proof, payer)?;
        let ixs = &[
            priority_fee::compute_budget_instruction(INIT_INSTRUCTION_BUDGET),
            priority_fee::compute_price_instruction_for_accounts(
                client,
                &ix.accounts,
                opts.fee_range(),
            )
            .await?,
            ix,
        ];
        message::mk_message(client, ixs, &opts.lut_addresses, payer).await
    }

    pub async fn init<E: AsEntityKey, C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
        client: &C,
        token: ClaimableToken,
        entity_key: &E,
        keypair: &Keypair,
        opts: &TransactionOpts,
    ) -> Result<(VersionedTransaction, u64), Error> {
        let (msg, block_height) =
            init_message(client, token, entity_key, &keypair.pubkey(), opts).await?;
        let txn = mk_transaction(msg, &[keypair])?;
        Ok((txn, block_height))
    }

    pub mod destination {
        use super::*;

        pub async fn for_kta<C: GetAnchorAccount + AsRef<DasClient>>(
            client: &C,
            token: ClaimableToken,
            kta: &helium_entity_manager::accounts::KeyToAssetV0,
        ) -> Result<Pubkey, Error> {
            let destination = super::for_kta(client, token, kta)
                .await?
                .map(|recipient| recipient.destination)
                .unwrap_or(Pubkey::default());
            if destination == Pubkey::default() {
                let asset = asset::for_kta(client, kta).await?;
                Ok(asset.ownership.owner)
            } else {
                Ok(destination)
            }
        }

        pub async fn for_ktas<C: GetAnchorAccount + AsRef<DasClient>>(
            client: &C,
            token: ClaimableToken,
            ktas: &[helium_entity_manager::accounts::KeyToAssetV0],
        ) -> Result<Vec<Pubkey>, Error> {
            // Get all recipients and map to destination accounts
            let mut maybe_destinations = super::for_ktas(client, token, ktas)
                .await?
                .into_iter()
                .map(|maybe_recipient| maybe_recipient.map(|recipient| recipient.destination))
                .collect_vec();
            // Find all None or default destinations and map the asset key to that index
            let asset_idxs: HashMap<Pubkey, usize> = ktas
                .iter()
                .zip(&maybe_destinations)
                .enumerate()
                .filter_map(
                    |(index, (kta, maybe_destination))| match maybe_destination {
                        None => Some((kta.asset, index)),
                        Some(pubkey) if pubkey == &Pubkey::default() => Some((kta.asset, index)),
                        _ => None,
                    },
                )
                .collect();
            // Get assets for None destinations
            let asset_keys = asset_idxs.keys().map(ToOwned::to_owned).collect_vec();
            let assets = asset::get_many(client, &asset_keys).await?;
            // Replace None or default destinations with the found asset owner
            assets.into_iter().for_each(|asset| {
                if let Some(recipient_index) = asset_idxs.get(&asset.id) {
                    if let Some(target) = maybe_destinations.get_mut(*recipient_index) {
                        *target = Some(asset.ownership.owner);
                    }
                }
            });

            Ok(maybe_destinations.into_iter().flatten().collect())
        }

        pub async fn for_entity_key<C: GetAnchorAccount + AsRef<DasClient>, E: AsEntityKey>(
            client: &C,
            token: ClaimableToken,
            entity_key: &E,
        ) -> Result<Pubkey, Error> {
            let kta = kta::for_entity_key(entity_key).await?;
            for_kta(client, token, &kta).await
        }

        pub async fn for_entity_keys<C: GetAnchorAccount + AsRef<DasClient>, E: AsEntityKey>(
            client: &C,
            token: ClaimableToken,
            entity_keys: &[E],
        ) -> Result<Vec<Pubkey>, Error> {
            let ktas = kta::for_entity_keys(entity_keys).await?;
            for_ktas(client, token, &ktas).await
        }

        pub async fn update_instruction(
            token: ClaimableToken,
            kta: &helium_entity_manager::accounts::KeyToAssetV0,
            asset: &asset::Asset,
            asset_proof: &asset::AssetProof,
            destination: &Pubkey,
        ) -> Result<Instruction, Error> {
            use lazy_distributor::{
                client::args::UpdateCompressionDestinationV0,
                types::UpdateCompressionDestinationArgsV0,
            };
            fn mk_accounts(
                owner: Pubkey,
                token: ClaimableToken,
                destination: Pubkey,
                merkle_tree: Pubkey,
                kta: &helium_entity_manager::accounts::KeyToAssetV0,
            ) -> impl ToAccountMetas {
                lazy_distributor::client::accounts::UpdateCompressionDestinationV0 {
                    owner,
                    destination,
                    recipient: token.recipient_key_from_kta(kta),
                    compression_program: spl_account_compression::ID,
                    merkle_tree,
                }
            }

            let mut accounts = mk_accounts(
                asset.ownership.owner,
                token,
                *destination,
                asset.compression.tree,
                kta,
            )
            .to_account_metas(None);
            accounts.extend_from_slice(&asset_proof.proof(Some(3))?);

            let data = UpdateCompressionDestinationV0 {
                args: UpdateCompressionDestinationArgsV0 {
                    data_hash: asset.compression.data_hash,
                    creator_hash: asset.compression.creator_hash,
                    root: asset_proof.root.to_bytes(),
                    index: asset.compression.leaf_id()?,
                },
            }
            .data();

            let ix = Instruction {
                program_id: lazy_distributor::ID,
                accounts,
                data,
            };
            Ok(ix)
        }

        pub async fn update_message<
            C: AsRef<SolanaRpcClient> + AsRef<DasClient>,
            E: AsEntityKey,
        >(
            client: &C,
            token: ClaimableToken,
            entity_key: &E,
            owner: &Pubkey,
            destination: &Pubkey,
            opts: &TransactionOpts,
        ) -> Result<(message::VersionedMessage, u64), Error> {
            let kta = kta::for_entity_key(entity_key).await?;
            let (asset, asset_proof) = asset::for_kta_with_proof(client, &kta).await?;
            let ix = update_instruction(token, &kta, &asset, &asset_proof, destination).await?;
            let ixs = &[
                priority_fee::compute_budget_instruction(200_000),
                priority_fee::compute_price_instruction_for_accounts(
                    client,
                    &ix.accounts,
                    opts.fee_range(),
                )
                .await?,
                ix,
            ];
            message::mk_message(client, ixs, &opts.lut_addresses, owner).await
        }

        pub async fn update<E: AsEntityKey, C: AsRef<SolanaRpcClient> + AsRef<DasClient>>(
            client: &C,
            token: ClaimableToken,
            entity_key: &E,
            destination: &Pubkey,
            keypair: &Keypair,
            opts: &TransactionOpts,
        ) -> Result<(VersionedTransaction, u64), Error> {
            let (msg, block_height) = update_message(
                client,
                token,
                entity_key,
                &keypair.pubkey(),
                destination,
                opts,
            )
            .await?;

            let txn = mk_transaction(msg, &[keypair])?;
            Ok((txn, block_height))
        }
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

    Ok(TokenAmount::from_u64(token, value))
}
