use crate::{cmd::*, result::Result, wallet::Wallet};
use prettytable::{format, Table};
use serde_json::json;

/// Verify an encypted wallet
#[derive(Debug, StructOpt)]
pub struct Cmd {}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let result = wallet.decrypt(password.as_bytes());
        print_result(&wallet, result.is_ok(), opts.format)
    }
}

pub fn print_result(wallet: &Wallet, result: bool, format: OutputFormat) -> Result {
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
            table.set_titles(row!["Address", "Sharded", "Verify", "PwHash"]);
            table.add_row(row![address, wallet.is_sharded(), result, wallet.pwhash()]);
            print_table(&table)
        }
        OutputFormat::Json => {
            let table = json!({
                "address": address,
                "sharded": wallet.is_sharded(),
                "verify": result,
                "pwhash": wallet.pwhash().to_string()
            });
            print_json(&table)
        }
    }
}
