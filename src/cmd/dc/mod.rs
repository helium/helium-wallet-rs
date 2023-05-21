use crate::{cmd::*, result::Result};

mod mint;
mod price;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: DcCommand,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts)
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
/// Commands on data credits
pub enum DcCommand {
    Price(price::Cmd),
    Mint(mint::Cmd),
}

impl DcCommand {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Price(cmd) => cmd.run(opts),
            Self::Mint(cmd) => cmd.run(opts),
        }
    }
}
