use crate::{cmd::*, result::Result};

mod generate;
mod list;
mod stake;
mod transfer;
mod unstake;

#[derive(Debug, StructOpt)]
/// Commands for validators
pub enum Cmd {
    /// Stake a validator with the given wallet as the owner.
    Stake(stake::Cmd),
    /// Unstake a validator
    Unstake(unstake::Cmd),
    /// Transfer a validator stake to a new validator and owner
    Transfer(Box<transfer::Cmd>),
    /// List all validators for one or more account addresses
    List(list::Cmd),
    /// Generate a validator key
    Generate(generate::Cmd),
}

impl Cmd {
    pub async fn run(self, opts: Opts) -> Result {
        match self {
            Self::Stake(cmd) => cmd.run(opts).await,
            Self::Unstake(cmd) => cmd.run(opts).await,
            Self::Transfer(cmd) => cmd.run(opts).await,
            Self::List(cmd) => cmd.run(opts).await,
            Self::Generate(cmd) => cmd.run(opts).await,
        }
    }
}
