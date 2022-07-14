use crate::{cmd::*, keypair::Keypair, result::Result, wallet::Wallet};
use prettytable::{Cell, Row, Table};
use serde_json::json;

/// Verify an encypted wallet
#[derive(Debug, StructOpt)]
pub struct Cmd {}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let decryped_wallet = wallet.decrypt(password.as_bytes());
        print_result(&wallet, &decryped_wallet, opts.format)
    }
}

pub fn print_result(
    wallet: &Wallet,
    decrypted_wallet: &Result<Keypair>,
    format: OutputFormat,
) -> Result {
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());
    let phrase = decrypted_wallet
        .as_ref()
        .map_or(Some(vec![]), |dw| dw.phrase().ok());

    let phrase_footnote = phrase.as_deref()
                                .filter(|p| p.len() == 12)
                                .map(|_x| String::from("* This 12-word phrase is output with bip39-compliant words even \n\
                                                          if they were once generated via mobile app. The last word may \n\
                                                          be different."));

    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.add_row(row!["Key", "Value"]);
            table.add_row(row!["Address", address]);
            table.add_row(row!["Sharded", wallet.is_sharded()]);
            table.add_row(row!["Verify", decrypted_wallet.is_ok()]);
            table.add_row(row!["PwHash", wallet.pwhash()]);
            if let Some(phrase) = phrase {
                let mut phrase_table = Table::new();
                phrase_table.set_format(*prettytable::format::consts::FORMAT_CLEAN);
                for segment in phrase.chunks(4) {
                    phrase_table.add_row(Row::new(segment.iter().map(|s| Cell::new(s)).collect()));
                }
                if phrase_footnote.is_some() {
                    table.add_row(row!["Phrase *", phrase_table]);
                } else {
                    table.add_row(row!["Phrase", phrase_table]);
                }
            }
            print_table(&table, phrase_footnote.as_ref())
        }
        OutputFormat::Json => {
            let mut table = json!({
                "address": address,
                "sharded": wallet.is_sharded(),
                "verify": decrypted_wallet.is_ok(),
                "pwhash": wallet.pwhash().to_string()
            });
            if let Some(phrase) = phrase {
                table["phrase"] = serde_json::Value::String(phrase.join(" "));
            }
            print_json(&table)
        }
    }
}
