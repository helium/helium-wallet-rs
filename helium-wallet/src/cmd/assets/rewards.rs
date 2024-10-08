use crate::cmd::*;
use helium_lib::{dao::SubDao, entity_key, reward};

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
    Current(CurrentCmd),
    Pending(PendingCmd),
}

impl RewardsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Current(cmd) => cmd.run(opts).await,
            Self::Pending(cmd) => cmd.run(opts).await,
        }
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List current (totel lifetime) rewards issued for a given entity key
pub struct CurrentCmd {
    /// Subdao for command
    subdao: SubDao,

    /// Entity key to look up
    entity_key: String,
}

impl CurrentCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let current = reward::current(&client, &self.subdao, &self.entity_key).await?;

        print_json(&current)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List claimable pending rewards for a given asset
pub struct PendingCmd {
    /// Subdao for command
    subdao: SubDao,

    /// Entity key to look up
    entity_key: String,
}

impl PendingCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let pending = reward::pending(
            &client,
            &self.subdao,
            &[self.entity_key.clone()],
            entity_key::KeySerialization::UTF8,
        )
        .await?;

        print_json(&pending)
    }
}
