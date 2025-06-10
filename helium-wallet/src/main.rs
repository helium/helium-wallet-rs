use clap::Parser;
use helium_wallet::{
    cmd::{
        assets, balance, burn, create, dc, export, hotspots, info, memo, price, router, sign,
        transfer, upgrade, Opts,
    },
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
    Price(price::Cmd),
    Transfer(transfer::Cmd),
    Burn(burn::Cmd),
    Export(export::Cmd),
    Sign(sign::Cmd),
    Memo(memo::Cmd),
    Assets(assets::Cmd),
}

#[allow(clippy::needless_return)]
#[tokio::main]
async fn main() -> Result {
    init();
    let cli = Cli::parse();
    cli.run().await
}

impl Cli {
    async fn run(self) -> Result {
        let client = self.opts.client()?;
        helium_lib::init(client.solana_client)?;
        match self.cmd {
            Cmd::Info(cmd) => cmd.run(self.opts).await,
            Cmd::Balance(cmd) => cmd.run(self.opts).await,
            Cmd::Upgrade(cmd) => cmd.run(self.opts).await,
            Cmd::Router(cmd) => cmd.run(self.opts).await,
            Cmd::Create(cmd) => cmd.run(self.opts).await,
            Cmd::Hotspots(cmd) => cmd.run(self.opts).await,
            Cmd::Dc(cmd) => cmd.run(self.opts).await,
            Cmd::Price(cmd) => cmd.run(self.opts).await,
            Cmd::Transfer(cmd) => cmd.run(self.opts).await,
            Cmd::Burn(cmd) => cmd.run(self.opts).await,
            Cmd::Export(cmd) => cmd.run(self.opts).await,
            Cmd::Sign(cmd) => cmd.run(self.opts).await,
            Cmd::Memo(cmd) => cmd.run(self.opts).await,
            Cmd::Assets(cmd) => cmd.run(self.opts).await,
        }
    }
}
