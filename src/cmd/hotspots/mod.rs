use crate::{cmd::*, result::Result};

// mod add;
// mod assert;
mod info;
mod list;
// mod transfer;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: HotspotCommand,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts)
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
/// Display list of hotspots associated with wallet
/// or transfer a hotspot to another wallet
pub enum HotspotCommand {
    // Add(add::Cmd),
    // Assert(Box<assert::Cmd>),
    List(list::Cmd),
    Info(info::Cmd),
    // Transfer(Box<transfer::Cmd>),
}

impl HotspotCommand {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            // Self::Add(cmd) => cmd.run(opts).await,
            // Self::Assert(cmd) => cmd.run(opts).await,
            Self::List(cmd) => cmd.run(opts),
            Self::Info(cmd) => cmd.run(opts),
            // Self::Transfer(cmd) => cmd.run(opts).await,
        }
    }
}
