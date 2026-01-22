use crate::cmd::*;
use helium_lib::{entity_key, reward, reward::ClaimableToken};

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

/// Inspect or claim rewards for assets
#[derive(Debug, Clone, clap::Subcommand)]
pub enum RewardsCommand {
    Claim(assets::claim::Cmd),
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

/// Manage the recipient for rewards
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientCmd {
    #[command(subcommand)]
    cmd: RecipientSubcommand,
}

impl RecipientCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum RecipientSubcommand {
    Get(RecipientGetCmd),
    Init(RecipientInitCmd),
    Update(RecipientUpdateCmd),
}

impl RecipientSubcommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Get(cmd) => cmd.run(opts).await,
            Self::Init(cmd) => cmd.run(opts).await,
            Self::Update(cmd) => cmd.run(opts).await,
        }
    }
}

/// Get the current reward recipient destination for an asset
///
/// Returns the wallet address where rewards for this asset will be sent
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientGetCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    /// The asset to get the reward recipient for
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
}

impl RecipientGetCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
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

/// Initialize the recipient for an asset
///
/// Creates the on-chain recipient account for an asset. This is required before
/// rewards can be claimed or a custom destination can be set. The recipient will
/// default to the asset owner's wallet.
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientInitCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    /// The asset to initialize the reward recipient for
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl RecipientInitCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let (tx, _) = reward::recipient::init(
            &client,
            self.token,
            &self.entity_key.as_entity_key()?,
            &keypair,
            &transaction_opts,
        )
        .await?;
        print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
    }
}

/// Update the reward recipient destination for an asset
///
/// Changes where rewards for this asset will be sent. The recipient account will
/// be initialized if it doesn't already exist.
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientUpdateCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    /// The asset to update the reward recipient for
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    /// The new destination wallet address to send rewards to
    pub destination: helium_lib::keypair::Pubkey,
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl RecipientUpdateCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let (tx, _) = reward::recipient::destination::update(
            &client,
            self.token,
            &self.entity_key.as_entity_key()?,
            &self.destination,
            &keypair,
            &transaction_opts,
        )
        .await?;
        print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
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
        let pending =
            reward::pending_amounts(&client, self.token, None, &[&self.entity_key]).await?;

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
