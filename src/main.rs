use helium_wallet::{
    cmd::{
        balance, burn, create, hotspots, htlc, info, multisig, oracle, oui, pay, request,
        securities, upgrade, validators, vars, verify, Opts,
    },
    result::Result,
};
use std::process;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
pub struct Cli {
    #[structopt(flatten)]
    opts: Opts,

    #[structopt(flatten)]
    cmd: Cmd,
}

#[derive(Debug, StructOpt)]
pub enum Cmd {
    Info(info::Cmd),
    Verify(verify::Cmd),
    Balance(balance::Cmd),
    Hotspots(Box<hotspots::Cmd>),
    Create(create::Cmd),
    Upgrade(upgrade::Cmd),
    Pay(pay::Cmd),
    Htlc(htlc::Cmd),
    Oui(oui::Cmd),
    Oracle(oracle::Cmd),
    Securities(securities::Cmd),
    Burn(burn::Cmd),
    Multisig(multisig::Cmd),
    Request(request::Cmd),
    Vars(vars::Cmd),
    Validators(validators::Cmd),
}

#[tokio::main]
async fn main() {
    let cli = Cli::from_args();
    if let Err(e) = run(cli).await {
        eprintln!("error: {:?}", e);
        process::exit(1);
    }
}

async fn run(cli: Cli) -> Result {
    match cli.cmd {
        Cmd::Info(cmd) => cmd.run(cli.opts).await,
        Cmd::Verify(cmd) => cmd.run(cli.opts).await,
        Cmd::Balance(cmd) => cmd.run(cli.opts).await,
        Cmd::Hotspots(cmd) => cmd.run(cli.opts).await,
        Cmd::Create(cmd) => cmd.run(cli.opts).await,
        Cmd::Upgrade(cmd) => cmd.run(cli.opts).await,
        Cmd::Pay(cmd) => cmd.run(cli.opts).await,
        Cmd::Htlc(cmd) => cmd.run(cli.opts).await,
        Cmd::Oui(cmd) => cmd.run(cli.opts).await,
        Cmd::Oracle(cmd) => cmd.run(cli.opts).await,
        Cmd::Securities(cmd) => cmd.run(cli.opts).await,
        Cmd::Burn(cmd) => cmd.run(cli.opts).await,
        Cmd::Multisig(cmd) => cmd.run(cli.opts).await,
        Cmd::Request(cmd) => cmd.run(cli.opts).await,
        Cmd::Vars(cmd) => cmd.run(cli.opts).await,
        Cmd::Validators(cmd) => cmd.run(cli.opts).await,
    }
}
