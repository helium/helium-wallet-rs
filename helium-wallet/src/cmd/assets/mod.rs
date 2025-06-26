use crate::cmd::*;

pub mod burn;
pub mod claim;
pub mod info;
pub mod rewards;
pub mod transfer;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: AssetCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
/// Commands on assets
pub enum AssetCommand {
    Rewards(rewards::Cmd),
    Info(info::Cmd),
    Burn(burn::Cmd),
    Transfer(transfer::Cmd),
}

impl AssetCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Rewards(cmd) => cmd.run(opts).await,
            Self::Info(cmd) => cmd.run(opts).await,
            Self::Burn(cmd) => cmd.run(opts).await,
            Self::Transfer(cmd) => cmd.run(opts).await,
        }
    }
}
