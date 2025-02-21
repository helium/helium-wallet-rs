use crate::cmd::*;

mod burn;
mod burn_delegated;
mod delegate;
mod mint;
mod price;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: DcCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
/// Commands on Data Credits
pub enum DcCommand {
    Price(price::Cmd),
    Mint(mint::Cmd),
    Delegate(delegate::Cmd),
    Burn(burn::Cmd),
    BurnDelegated(burn_delegated::Cmd),
}

impl DcCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Price(cmd) => cmd.run(opts).await,
            Self::Mint(cmd) => cmd.run(opts).await,
            Self::Delegate(cmd) => cmd.run(opts).await,
            Self::Burn(cmd) => cmd.run(opts).await,
            Self::BurnDelegated(cmd) => cmd.run(opts).await,
        }
    }
}
