use crate::{
    cmd::{get_password, load_wallet, Opts, OutputFormat},
    result::Result,
    wallet::Wallet,
};
use prettytable::{format, Table};
use serde_json::json;
use structopt::StructOpt;

/// Verify an encypted wallet
#[derive(Debug, StructOpt)]
pub struct Cmd {}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let result = wallet.to_keypair(password.as_bytes());
        print_result(&wallet, result.is_ok(), opts.format)
    }
}

pub fn print_result(wallet: &Wallet, result: bool, format: OutputFormat) -> Result {
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
            table.set_titles(row!["Address", "Sharded", "Verify"]);
            table.add_row(row![address, wallet.is_sharded(), result]);
            table.printstd();
            Ok(())
        }
        OutputFormat::Json => {
            let table = json!({
                "address": address,
                "sharded": wallet.is_sharded(),
                "verify": result
            });
            println!("{}", serde_json::to_string_pretty(&table)?);
            Ok(())
        }
    }
}
