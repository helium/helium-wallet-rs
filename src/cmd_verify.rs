use crate::{result::Result, wallet::Wallet};
use prettytable::{format, Table};

pub fn cmd_verify(wallet: &Wallet, password: &str) -> Result {
    let result = wallet.to_keypair(password.as_bytes());
    print_result(wallet, result.is_ok());
    Ok(())
}

fn print_result(wallet: &Wallet, result: bool) {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
    table.set_titles(row!["Address", "Sharded", "Verify", "Seed"]);
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());
    table.add_row(row![address, wallet.is_sharded(), result]);
    table.printstd();
}
