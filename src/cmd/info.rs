use crate::{
    cmd::{load_wallet, Opts},
    result::Result,
    wallet::Wallet,
};
use prettytable::Table;
use qr2term::print_qr;
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
        } else {
            print_wallet(&wallet);
        }
        Ok(())
    }
}

pub fn cmd_info(wallet: &Wallet, qr_code: bool) -> Result {
    if qr_code {
        let address = wallet.address()?;
        print_qr(&address)?;
    } else {
        print_wallet(wallet);
    }
    Ok(())
}

fn print_wallet(wallet: &Wallet) {
    let mut table = Table::new();
    table.add_row(row!["Address", "Sharded"]);
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());
    table.add_row(row![address, wallet.is_sharded()]);
    table.printstd();
}
