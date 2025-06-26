use crate::cmd::*;
use anyhow::Context;
use helium_lib::{entity_key, reward, reward::ClaimableToken, token::TokenAmount};

#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: RewardsCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum RewardsCommand {
    Claim(ClaimCmd),
    Pending(PendingCmd),
    Recipient(RecipientCmd),
    Lifetime(LifetimeCmd),
    MaxClaim(MaxClaimCmd),
}

impl RewardsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Claim(cmd) => cmd.run(opts).await,
            Self::Recipient(cmd) => cmd.run(opts).await,
            Self::MaxClaim(cmd) => cmd.run(opts).await,
            Self::Pending(cmd) => cmd.run(opts).await,
            Self::Lifetime(cmd) => cmd.run(opts).await,
        }
    }
}

/// Claim rewards for a given hotspot
#[derive(Debug, Clone, clap::Args)]
pub struct ClaimCmd {
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

impl ClaimCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);

        let token_amount = self
            .amount
            .map(|amount| TokenAmount::from_f64(self.token, amount).amount);
        let Some((tx, _)) = reward::claim(
            &client,
            self.token,
            token_amount,
            &self.entity_key,
            &keypair,
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

/// Get or set the recipient for rewards
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    /// The asset to get or set the reward recipient for
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    /// The new destination to send rewards to, if set
    pub destination: Option<helium_lib::keypair::Pubkey>,
    /// Commit the new destination if set
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl RecipientCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);

        if let Some(destination) = self.destination {
            let password = get_wallet_password(false)?;
            let keypair = opts.load_keypair(password.as_bytes())?;
            let (tx, _) = reward::recipient::destination::update(
                &client,
                self.token,
                &self.entity_key.as_entity_key()?,
                &destination,
                &keypair,
                &transaction_opts,
            )
            .await?;
            print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
        } else {
            let destination = reward::recipient::destination::for_entity_key(
                &client,
                self.token,
                &self.entity_key.as_entity_key()?,
            )
            .await?;
            let json = json!({
                "destination": destination.to_string(),
            });
            print_json(&json)
        }
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List the maximum claim amount for the given subdao
///
/// The max claim amoount is the configured threshold for the subdao, adjusted down by a time
/// decayed amount bed on previous claims
pub struct MaxClaimCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: ClaimableToken,
}

impl MaxClaimCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let max_claim = reward::max_claim(&client, self.token).await?;
        print_json(&max_claim)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List claimable pending rewards for a given asset
pub struct PendingCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: ClaimableToken,
    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
}

impl PendingCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let pending = reward::pending(&client, self.token, None, &[&self.entity_key]).await?;

        print_json(&pending)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List lifetime rewards for an asset
///
/// This includes both claimed and unclaimed rewards
pub struct LifetimeCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: ClaimableToken,
    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
}

impl LifetimeCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let rewards = reward::lifetime(&client, self.token, &[&self.entity_key]).await?;

        print_json(&rewards)
    }
}
