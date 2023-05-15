use clap::Parser;
use helium_wallet::{
    cmd::{balance, create, dc, hotspots, info, router, upgrade, verify, Opts},
    result::Result,
};
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
    Verify(verify::Cmd),
    Upgrade(upgrade::Cmd),
    Router(router::Cmd),
    Create(create::Cmd),
    Hotspots(Box<hotspots::Cmd>),
    Dc(dc::Cmd),
    // Pay(Box<pay::Cmd>),
    // Htlc(htlc::Cmd),
    // Oui(oui::Cmd),
    // Oracle(oracle::Cmd),
    // Burn(burn::Cmd),
    // Multisig(multisig::Cmd),
    // Request(request::Cmd),
    // Vars(vars::Cmd),
    // Validators(validators::Cmd),
    // Commit(commit::Cmd),
    // Sign(sign::Cmd),
    // Export(export::Cmd),
}

fn main() -> Result {
    let cli = Cli::parse();
    run(cli)
}

fn run(cli: Cli) -> Result {
    match cli.cmd {
        Cmd::Info(cmd) => cmd.run(cli.opts),
        Cmd::Balance(cmd) => cmd.run(cli.opts),
        Cmd::Verify(cmd) => cmd.run(cli.opts),
        Cmd::Upgrade(cmd) => cmd.run(cli.opts),
        Cmd::Router(cmd) => cmd.run(cli.opts),
        Cmd::Create(cmd) => cmd.run(cli.opts),
        Cmd::Hotspots(cmd) => cmd.run(cli.opts),
        Cmd::Dc(cmd) => cmd.run(cli.opts),
        // Cmd::Pay(cmd) => cmd.run(cli.opts),
        // Cmd::Htlc(cmd) => cmd.run(cli.opts),
        // Cmd::Oracle(cmd) => cmd.run(cli.opts),
        // Cmd::Burn(cmd) => cmd.run(cli.opts),
        // Cmd::Multisig(cmd) => cmd.run(cli.opts),
        // Cmd::Request(cmd) => cmd.run(cli.opts),
        // Cmd::Vars(cmd) => cmd.run(cli.opts),
        // Cmd::Validators(cmd) => cmd.run(cli.opts),
        // Cmd::Commit(cmd) => cmd.run(cli.opts),
        // Cmd::Sign(cmd) => cmd.run(cli.opts),
        // Cmd::Export(cmd) => cmd.run(cli.opts),
    }
}
