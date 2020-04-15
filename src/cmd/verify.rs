use crate::{
    cmd::{Opts, load_wallet, get_password},
    result::Result,
    wallet::Wallet,
};
use structopt::StructOpt;
use prettytable::{format, Table};

/// Verify an encypted wallet
#[derive(Debug, StructOpt)]
pub struct Cmd { }

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let result = wallet.to_keypair(password.as_bytes());
        print_result(&wallet, result.is_ok());
        Ok(())
    }
}

fn print_result(wallet: &Wallet, result: bool) {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
    table.set_titles(row!["Address", "Sharded", "Verify", "Seed"]);
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());
    table.add_row(row![address, wallet.is_sharded(), result]);
    table.printstd();
}
