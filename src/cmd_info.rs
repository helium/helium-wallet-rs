use crate::{
    traits::{ReadWrite, B58},
    wallet::Wallet,
};
use prettytable::Table;
use qr2term::print_qr;
use std::{error::Error, fs, path::PathBuf, result::Result};

pub fn cmd_info(files: Vec<PathBuf>, qr_code: bool) -> Result<(), Box<dyn Error>> {
    let mut wallets = Vec::with_capacity(files.len());
    for file in files {
        let mut reader = fs::File::open(&file)?;
        let enc_wallet = Wallet::read(&mut reader)?;
        wallets.push(enc_wallet);
    }
    if wallets.len() == 1 && qr_code {
        let address = wallets[0].public_key().to_b58()?;
        print_qr(&address)?;
    } else {
        print_wallets(wallets);
    }
    Ok(())
}

fn print_wallets(wallets: Vec<Wallet>) {
    let mut table = Table::new();
    table.add_row(row!["Address", "Sharded"]);
    for wallet in wallets {
        let address = wallet
            .public_key()
            .to_b58()
            .unwrap_or_else(|_| "unknown".to_string());
        table.add_row(row![address, wallet.is_sharded()]);
    }
    table.printstd();
}
