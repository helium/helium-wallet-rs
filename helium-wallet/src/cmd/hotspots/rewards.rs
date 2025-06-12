use crate::cmd::*;
use client::DasClient;
use helium_lib::{entity_key::EncodedEntityKey, hotspot, keypair::Pubkey, reward};

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
    Pending(PendingCmd),
    Lifetime(LifetimeCmd),
    Claim(ClaimCmd),
    Recipient(RecipientCmd),
}

impl RewardsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Pending(cmd) => cmd.run(opts).await,
            Self::Lifetime(cmd) => cmd.run(opts).await,
            Self::Claim(cmd) => cmd.run(opts).await,
            Self::Recipient(cmd) => cmd.run(opts).await,
        }
    }
}

async fn collect_hotspots<C: AsRef<DasClient>>(
    client: &C,
    hotspots: Option<Vec<helium_crypto::PublicKey>>,
    owner: Option<Pubkey>,
) -> Result<Vec<helium_crypto::PublicKey>> {
    if let Some(list) = hotspots {
        Ok(list)
    } else if let Some(owner) = owner {
        let hotspots = hotspot::for_owner(&client, &owner)
            .await?
            .into_iter()
            .map(|hotspot| hotspot.key)
            .collect::<Vec<helium_crypto::PublicKey>>();
        Ok(hotspots)
    } else {
        Ok(vec![])
    }
}

#[derive(Clone, Debug, clap::Args)]
/// List pending rewards for given Hotspots
pub struct PendingCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: reward::ClaimableToken,
    /// Hotspots to lookup
    hotspots: Option<Vec<helium_crypto::PublicKey>>,
    /// Wallet to look up hotspots for
    #[arg(long)]
    owner: Option<Pubkey>,
}

impl PendingCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let owner = opts.maybe_wallet_key(self.owner)?;
        let hotspots = collect_hotspots(&client, self.hotspots.clone(), Some(owner)).await?;
        let encoded_entity_keys: Vec<EncodedEntityKey> = hotspots.iter().map(Into::into).collect();
        let pending =
            reward::pending_amounts(&client, self.token, None, &encoded_entity_keys).await?;

        print_json(&pending)
    }
}

#[derive(Clone, Debug, clap::Args)]
/// List lifetime total rewards for a hotspot
///
/// This includes both claimed and unclaimed rewards
pub struct LifetimeCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: reward::ClaimableToken,
    /// Hotspots to lookup
    hotspots: Option<Vec<helium_crypto::PublicKey>>,
    /// Wallet to look up hotspots for
    #[arg(long)]
    owner: Option<Pubkey>,
}

impl LifetimeCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let owner = opts.maybe_wallet_key(self.owner)?;
        let hotspots = collect_hotspots(&client, self.hotspots.clone(), Some(owner)).await?;
        let encoded_entity_keys: Vec<EncodedEntityKey> = hotspots.iter().map(Into::into).collect();
        let rewards = reward::lifetime(&client, self.token, &encoded_entity_keys).await?;

        print_json(&rewards)
    }
}

#[derive(Clone, Debug, clap::Args)]
/// Claim rewards for one or all Hotspots in a wallet
pub struct ClaimCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: reward::ClaimableToken,
    /// Hotspot public key to send claim for
    hotspot: helium_crypto::PublicKey,
    /// The optional amount to claim
    ///
    /// If not specific the full pending amount is claimed, limited by the maximum
    /// claim amount for the subdao
    pub amount: Option<f64>,
    /// Commit the claim transaction.
    #[command(flatten)]
    commit: CommitOpts,
}

impl From<&ClaimCmd> for crate::cmd::assets::claim::one::Cmd {
    fn from(value: &ClaimCmd) -> Self {
        Self {
            token: value.token,
            entity_key: EncodedEntityKey::from(&value.hotspot),
            amount: value.amount,
            commit: value.commit.clone(),
        }
    }
}

impl ClaimCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let cmd = crate::cmd::assets::claim::one::Cmd::from(self);
        cmd.run(opts).await
    }
}

/// Get or set the recipient for hotspot rewards
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: reward::ClaimableToken,
    /// The hotspot to get or set the reward recipient for
    pub hotspot: helium_crypto::PublicKey,
    /// The new destination to send rewards to, if set
    pub destination: Option<helium_lib::keypair::Pubkey>,
    /// Commit the new destination if set
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl From<&RecipientCmd> for crate::cmd::assets::rewards::RecipientCmd {
    fn from(value: &RecipientCmd) -> Self {
        Self {
            token: value.token,
            entity_key: EncodedEntityKey::from(&value.hotspot),
            destination: value.destination,
            commit: value.commit.clone(),
        }
    }
}

impl RecipientCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let cmd = crate::cmd::assets::rewards::RecipientCmd::from(self);
        cmd.run(opts).await
    }
}
