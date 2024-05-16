use crate::cmd::*;

mod add;
mod info;
mod list;
mod rewards;
mod update;
mod updates;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: HotspotCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
/// Commands on hotspots
pub enum HotspotCommand {
    Update(update::Cmd),
    Add(add::Cmd),
    List(list::Cmd),
    Info(info::Cmd),
    Updates(updates::Cmd),
    Rewards(rewards::Cmd),
    // Transfer(Box<transfer::Cmd>),
}

impl HotspotCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Update(cmd) => cmd.run(opts).await,
            Self::Add(cmd) => cmd.run(opts).await,
            Self::List(cmd) => cmd.run(opts).await,
            Self::Info(cmd) => cmd.run(opts).await,
            Self::Updates(cmd) => cmd.run(opts).await,
            Self::Rewards(cmd) => cmd.run(opts).await,
            // Self::Transfer(cmd) => cmd.run(opts).await,
        }
    }
}
