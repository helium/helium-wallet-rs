use crate::cmd::*;
use anyhow::Context;
use helium_lib::{entity_key, reward, reward::ClaimableToken, token::TokenAmount};

/// Claim rewards for a single asset
#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    /// The optional amount to claim
    ///
    /// If not specific the full pending amount is claimed, limited by the maximum
    /// claim amount for the subdao
    pub amount: Option<f64>,
    /// Commit the claim transaction.
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);

        let token_amount = self
            .amount
            .map(|amount| TokenAmount::from_f64(self.token, amount).map(|ta| ta.amount))
            .transpose()?;
        let Some((tx, _)) = reward::claim(
            &client,
            self.token,
            token_amount,
            &self.entity_key,
            &*signer,
            &transaction_opts,
        )
        .await?
        else {
            bail!("No rewards to claim")
        };

        let claim_response = self
            .commit
            .maybe_commit(tx, &client)
            .await
            .context("while claiming rewards")?;
        print_json(&claim_response.to_json())
    }
}
