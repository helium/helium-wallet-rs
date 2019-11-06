use crate::{
    result::Result,
    traits::{ReadWrite, B58},
    wallet::{self, Wallet},
};
use prettytable::{format, Table};
use std::{fs, path::PathBuf};

pub fn cmd_verify(files: Vec<PathBuf>, password: &str) -> Result {
    let first_file = files.first().expect("At least one file expected");
    let first_wallet = {
        let mut reader = fs::File::open(first_file)?;
        Wallet::read(&mut reader)?
    };

    let mut results: Vec<(Wallet, Result<Wallet>)> = Vec::with_capacity(files.len());
    if first_wallet.is_sharded() {
        let mut enc_wallets = Vec::new();
        for file in files.iter() {
            let mut reader = fs::File::open(&file)?;
            let enc_wallet = Wallet::read(&mut reader)?;
            enc_wallets.push(enc_wallet);
        }
        let result = wallet::Wallet::decrypt_sharded(password.as_bytes(), &enc_wallets);
        results.push((first_wallet, result));
    } else {
        let file = files.first().expect("Missing wallet filename");
        let mut reader = fs::File::open(&file)?;
        let enc_wallet = Wallet::read(&mut reader)?;
        let result = wallet::Wallet::decrypt_basic(password.as_bytes(), &enc_wallet);
        results.push((enc_wallet, result));
    };
    print_results(results);
    Ok(())
}

fn print_results(results: Vec<(Wallet, Result<Wallet>)>) {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
    table.set_titles(row!["Address", "Sharded", "Verify", "Seed"]);
    for (enc_wallet, result) in results {
        let address = enc_wallet
            .public_key()
            .to_b58()
            .unwrap_or_else(|_| "unknown".to_string());
        table.add_row(row![address, enc_wallet.is_sharded(), result.is_ok()]);
    }
    table.printstd();
}
