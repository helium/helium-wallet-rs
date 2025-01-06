use crate::cmd::*;
use client::DasClient;
use helium_lib::{
    entity_key::{EncodedEntityKey, KeySerialization},
    hotspot,
    keypair::Pubkey,
    reward,
};

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
}

impl RewardsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Pending(cmd) => cmd.run(opts).await,
            Self::Lifetime(cmd) => cmd.run(opts).await,
            Self::Claim(cmd) => cmd.run(opts).await,
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
        let wallet = opts.load_wallet()?;
        let hotspots = collect_hotspots(
            &client,
            self.hotspots.clone(),
            self.owner.or(Some(wallet.public_key)),
        )
        .await?;
        let entity_key_strings = hotspots_to_entity_key_strings(&hotspots);
        let pending = reward::pending(
            &client,
            self.token,
            &entity_key_strings,
            KeySerialization::B58,
        )
        .await?;

        print_json(&pending)
    }
}

#[derive(Clone, Debug, clap::Args)]
/// List lifetime total rewards for a hotspot
///
/// This includes both claimed and unclaimed rewards
pub struct LifetimeCmd {
    /// Token for command
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
        let wallet = opts.load_wallet()?;
        let hotspots = collect_hotspots(
            &client,
            self.hotspots.clone(),
            self.owner.or(Some(wallet.public_key)),
        )
        .await?;
        let entity_key_strings = hotspots_to_entity_key_strings(&hotspots);
        let rewards = reward::lifetime(&client, self.token, &entity_key_strings).await?;

        print_json(&rewards)
    }
}

#[derive(Clone, Debug, clap::Args)]
/// Claim rewards for one or all Hotspots in a wallet
pub struct ClaimCmd {
    /// Token for command
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

impl From<&ClaimCmd> for crate::cmd::assets::rewards::ClaimCmd {
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
        let cmd = crate::cmd::assets::rewards::ClaimCmd::from(self);
        cmd.run(opts).await
    }
}

fn hotspots_to_entity_key_strings(public_keys: &[helium_crypto::PublicKey]) -> Vec<String> {
    public_keys
        .iter()
        .map(|key| key.to_string())
        .collect::<Vec<String>>()
}
