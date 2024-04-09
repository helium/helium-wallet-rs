use crate::cmd::*;

pub mod balance;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: RouterCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

/// Operations on routers
#[derive(Debug, clap::Subcommand)]
pub enum RouterCommand {
    Balance(balance::Cmd),
}

impl RouterCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            RouterCommand::Balance(cmd) => cmd.run(opts).await,
        }
    }
}
