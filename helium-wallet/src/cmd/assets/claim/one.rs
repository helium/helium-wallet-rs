use crate::cmd::*;
use anyhow::Context;
use helium_lib::{
    blockchain_api::types::{ClaimHotspotRewardsRequest, RewardNetwork},
    entity_key,
    keypair::Signer,
    reward::ClaimableToken,
};

/// Claim rewards for a single asset
///
/// Claims the full pending amount; partial-amount claims are not supported.
#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    /// Commit the claim transaction.
    #[command(flatten)]
    pub commit: CommitOpts,
}

fn network_for(token: ClaimableToken) -> RewardNetwork {
    match token {
        ClaimableToken::Iot => RewardNetwork::Iot,
        ClaimableToken::Mobile => RewardNetwork::Mobile,
        ClaimableToken::Hnt => RewardNetwork::Hnt,
    }
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .claim_hotspot_rewards(
                &self.entity_key.to_string(),
                &ClaimHotspotRewardsRequest {
                    wallet_address: signer.pubkey().to_string(),
                    network: Some(network_for(self.token)),
                },
            )
            .await?;
        let claim_response = self
            .commit
            .commit_via_api(&api, &client, &response, &*signer)
            .await
            .context("while claiming rewards")?;
        print_json(&claim_response.to_json())
    }
}
