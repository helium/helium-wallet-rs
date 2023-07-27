use crate::{cmd::*, result::Result};

mod add;
mod assert;
mod info;
mod list;

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
/// Commands on hotspots
pub enum HotspotCommand {
    Assert(assert::Cmd),
    Add(add::Cmd),
    List(list::Cmd),
    Info(info::Cmd),
    // Transfer(Box<transfer::Cmd>),
}

impl HotspotCommand {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Assert(cmd) => cmd.run(opts),
            Self::Add(cmd) => cmd.run(opts),
            Self::List(cmd) => cmd.run(opts),
            Self::Info(cmd) => cmd.run(opts),
            // Self::Transfer(cmd) => cmd.run(opts).await,
        }
    }
}
