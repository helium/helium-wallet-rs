use clap::Parser;
use helium_wallet::{
    cmd::{balance, create, dc, export, hotspots, info, router, sign, transfer, upgrade, Opts},
    result::Result,
};

static START: std::sync::Once = std::sync::Once::new();

fn init() {
    START.call_once(|| sodiumoxide::init().expect("Failed to intialize sodium"))
}

#[derive(Debug, Parser)]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(name = env!("CARGO_BIN_NAME"))]
pub struct Cli {
    #[command(flatten)]
    opts: Opts,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum Cmd {
    Info(info::Cmd),
    Balance(balance::Cmd),
    Upgrade(upgrade::Cmd),
    Router(router::Cmd),
    Create(create::Cmd),
    Hotspots(Box<hotspots::Cmd>),
    Dc(dc::Cmd),
    Transfer(transfer::Cmd),
    Export(export::Cmd),
    Sign(sign::Cmd),
    // Assets(assets::Cmd),
}

#[tokio::main]
async fn main() -> Result {
    init();
    let cli = Cli::parse();
    run(cli).await
}

async fn run(cli: Cli) -> Result {
    match cli.cmd {
        Cmd::Info(cmd) => cmd.run(cli.opts).await,
        Cmd::Balance(cmd) => cmd.run(cli.opts).await,
        Cmd::Upgrade(cmd) => cmd.run(cli.opts).await,
        Cmd::Router(cmd) => cmd.run(cli.opts).await,
        Cmd::Create(cmd) => cmd.run(cli.opts).await,
        Cmd::Hotspots(cmd) => cmd.run(cli.opts).await,
        Cmd::Dc(cmd) => cmd.run(cli.opts).await,
        Cmd::Transfer(cmd) => cmd.run(cli.opts).await,
        Cmd::Export(cmd) => cmd.run(cli.opts).await,
        Cmd::Sign(cmd) => cmd.run(cli.opts).await,
        // Cmd::Assets(cmd) => cmd.run(cli.opts).await,
    }
}
