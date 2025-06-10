use crate::cmd::*;

pub mod one;
pub mod queue;

#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: ClaimCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

/// Claim rewards for assets
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ClaimCommand {
    One(one::Cmd),
    Queue(queue::Cmd),
}

impl ClaimCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::One(cmd) => cmd.run(opts).await,
            Self::Queue(cmd) => cmd.run(opts).await,
        }
    }
}
