use crate::{
    asset, circuit_breaker,
    client::{DasClient, GetAnchorAccount},
    entity_key::{AsEntityKey, EncodedEntityKey},
    error::{DecodeError, Error},
    helium_entity_manager,
    keypair::Pubkey,
    kta, lazy_distributor,
    token::{Token, TokenAmount},
};
use chrono::Utc;
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use itertools::{izip, Itertools};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Display};

/// A reward oracle that provides lifetime reward amounts for Helium entities.
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

/// A reward amount reported by a specific oracle, identified by oracle index.
#[derive(Debug, Serialize, Clone)]
pub struct OracleReward {
    pub oracle: Oracle,
    pub index: u16,
    pub reward: TokenAmount,
}

/// Helium tokens that are eligible for reward claiming via the lazy distributor.
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum ClaimableToken {
    /// IoT network token rewards.
    Iot,
    /// Mobile network token rewards.
    Mobile,
    /// HNT token rewards (default).
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

/// Fetches the on-chain lazy distributor account for a claimable token.
///
/// The lazy distributor is the mechanism that distributes Helium rewards on-chain
/// using oracle-signed reward amounts.
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

/// Derives the circuit breaker PDA for a lazy distributor's rewards escrow.
///
/// The circuit breaker rate-limits reward distribution to prevent excessive payouts.
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

/// Returns the maximum claimable amount, bounded by the circuit breaker's windowed threshold.
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

/// Returns pending (unclaimed) reward amounts for multiple entities as a simplified map.
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

/// Returns pending (unclaimed) rewards for multiple entities with full oracle details.
///
/// Computes pending = lifetime - already_claimed for each entity, using the median oracle value.
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

/// Fetches lifetime reward totals from all oracles for the given entities.
pub async fn lifetime<C: GetAnchorAccount, E: AsRef<EncodedEntityKey>>(
    client: &C,
    token: ClaimableToken,
    encoded_entity_keys: &[E],
) -> Result<HashMap<String, Vec<OracleReward>>, Error> {
    let ld_account = lazy_distributor(client, token).await?;
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

/// Reward recipient accounts -- tracks total claimed rewards and optional custom destinations.
pub mod recipient {
    use super::*;

    /// Fetches the recipient account for a key-to-asset, if one exists.
    pub async fn for_kta<C: GetAnchorAccount>(
        client: &C,
        token: ClaimableToken,
        kta: &helium_entity_manager::accounts::KeyToAssetV0,
    ) -> Result<Option<lazy_distributor::accounts::RecipientV0>, Error> {
        let recipient_key = token.recipient_key_from_kta(kta);
        Ok(client.anchor_account(&recipient_key).await.ok())
    }

    /// Fetches the recipient account for an entity key, if one exists.
    pub async fn for_entity_key<C: GetAnchorAccount, E: AsEntityKey>(
        client: &C,
        token: ClaimableToken,
        entity_key: &E,
    ) -> Result<Option<lazy_distributor::accounts::RecipientV0>, Error> {
        let kta = kta::for_entity_key(entity_key).await?;
        for_kta(client, token, &kta).await
    }

    /// Fetches recipient accounts for multiple key-to-asset entries in batch.
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

    /// Fetches recipient accounts for multiple entity keys in batch.
    pub async fn for_entity_keys<C: GetAnchorAccount, E: AsEntityKey>(
        client: &C,
        token: ClaimableToken,
        entity_keys: &[E],
    ) -> Result<Vec<Option<lazy_distributor::accounts::RecipientV0>>, Error> {
        let ktas = kta::for_entity_keys(entity_keys).await?;
        for_ktas(client, token, &ktas).await
    }

    pub mod destination {
        use super::*;

        /// Returns the reward destination for a key-to-asset (custom destination or asset owner).
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

        /// Returns reward destinations for multiple key-to-asset entries in batch.
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

        /// Returns the reward destination for a single entity key.
        pub async fn for_entity_key<C: GetAnchorAccount + AsRef<DasClient>, E: AsEntityKey>(
            client: &C,
            token: ClaimableToken,
            entity_key: &E,
        ) -> Result<Pubkey, Error> {
            let kta = kta::for_entity_key(entity_key).await?;
            for_kta(client, token, &kta).await
        }

        /// Returns reward destinations for multiple entity keys in batch.
        pub async fn for_entity_keys<C: GetAnchorAccount + AsRef<DasClient>, E: AsEntityKey>(
            client: &C,
            token: ClaimableToken,
            entity_keys: &[E],
        ) -> Result<Vec<Pubkey>, Error> {
            let ktas = kta::for_entity_keys(entity_keys).await?;
            for_ktas(client, token, &ktas).await
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
