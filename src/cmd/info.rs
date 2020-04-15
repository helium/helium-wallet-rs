use crate::{
    cmd::{load_wallet, Opts, OutputFormat},
    result::Result,
    wallet::Wallet,
};
use prettytable::Table;
use qr2term::print_qr;
use serde_json::json;
use structopt::StructOpt;

/// Get wallet information
#[derive(Debug, StructOpt)]
pub struct Cmd {
    /// Display QR code for a given single wallet.
    #[structopt(long = "qr")]
    qr_code: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;
        if self.qr_code {
            let address = wallet.address()?;
            print_qr(&address)?;
            Ok(())
        } else {
            print_wallet(&wallet, opts.format)
        }
    }
}

fn print_wallet(wallet: &Wallet, format: OutputFormat) -> Result {
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.add_row(row!["Address", "Sharded"]);
            table.add_row(row![address, wallet.is_sharded()]);
            table.printstd();
        }
        OutputFormat::Json => {
            let table = json!({
                "address": address,
                "sharded": wallet.is_sharded()
            });
            println!("{}", serde_json::to_string_pretty(&table)?);
        }
    };
    Ok(())
}
