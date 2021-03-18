use crate::{cmd::*, result::Result, wallet::Wallet};
use helium_api::accounts::{self, Account};
use prettytable::Table;
use qr2term::print_qr;
use serde_json::json;

/// Get wallet information
#[derive(Debug, StructOpt)]
pub struct Cmd {
    /// Display QR code for a given single wallet.
    #[structopt(long = "qr")]
    qr_code: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;
        if self.qr_code {
            let address = wallet.address()?;
            print_qr(&address)?;
            Ok(())
        } else {
            let client = Client::new_with_base_url(api_url(wallet.public_key.network));
            let account = accounts::get(&client, &wallet.address()?).await?;
            print_wallet(&wallet, &account, opts.format)
        }
    }
}

fn print_wallet(wallet: &Wallet, account: &Account, format: OutputFormat) -> Result {
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.add_row(row!["Key", "Value"]);
            table.add_row(row!["Address", account.address]);
            table.add_row(row!["Network", wallet.public_key.tag().network]);
            table.add_row(row!["Type", wallet.public_key.tag().key_type]);
            table.add_row(row!["Sharded", wallet.is_sharded()]);
            table.add_row(row!["PwHash", wallet.pwhash()]);
            table.add_row(row!["Balance", account.balance]);
            table.add_row(row!["DC Balance", account.dc_balance]);
            table.add_row(row!["Securities Balance", account.sec_balance]);
            print_table(&table)
        }
        OutputFormat::Json => {
            let table = json!({
                "sharded": wallet.is_sharded(),
                "network": wallet.public_key.tag().network.to_string(),
                "type": wallet.public_key.tag().key_type.to_string(),
                "pwhash": wallet.pwhash().to_string(),
                "account": account,
            });
            print_json(&table)
        }
    }
}
