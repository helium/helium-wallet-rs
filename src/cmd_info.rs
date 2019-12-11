use crate::{result::Result, wallet::Wallet};
use prettytable::Table;
use qr2term::print_qr;

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
    table.add_row(row!["Address", "Type"]);
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());

    let wallet_type = if wallet.is_sharded() {
        "Sharded"
    } else {
        "Monolithic"
    };

    table.add_row(row![address, wallet_type]);
    table.printstd();
}
