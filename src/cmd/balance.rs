use crate::{
    cmd::*,
    keypair::PublicKey,
    result::{anyhow, Result},
};
use helium_api::{accounts, models::Account};
use prettytable::{format, Table};
use serde_json::json;

#[derive(Debug, StructOpt)]
/// Get the balance for a wallet. The balance is given in HNT and has
/// a precision of 8 decimals.
pub struct Cmd {
    /// Addresses to get balances for
    #[structopt(short = "a", long = "address")]
    addresses: Vec<PublicKey>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let addresses = collect_addresses(opts.files, self.addresses.clone())?;
        let api_url = api_url(
            addresses
                .first()
                .map(|key| key.network)
                .ok_or_else(|| anyhow!("at least one address expected"))?,
        );
        let client = new_client(api_url);

        let mut results = Vec::with_capacity(self.addresses.len());
        for address in addresses {
            results.push((
                address.to_string(),
                accounts::get(&client, &address.to_string())
                    .await
                    .map_err(|e| e.into()),
            ));
        }
        print_results(results, opts.format)
    }
}

fn print_results(results: Vec<(String, Result<Account>)>, format: OutputFormat) -> Result {
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
            table.set_titles(row![
                "Address",
                "Balance",
                "Staked Balance",
                "Data Credits",
                "Security Tokens"
            ]);
            for (address, result) in results {
                match result {
                    Ok(account) => table.add_row(row![
                        address,
                        account.balance,
                        account.staked_balance,
                        account.dc_balance,
                        account.sec_balance
                    ]),
                    Err(err) => table.add_row(row![address, H3 -> err.to_string()]),
                };
            }
            print_table(&table)
        }
        OutputFormat::Json => {
            let mut rows = Vec::with_capacity(results.len());
            for (address, result) in results {
                if let Ok(account) = result {
                    rows.push(json!({
                        "address": address,
                        "dc_balance": account.dc_balance,
                        "staked_balance": account.staked_balance,
                        "sec_balance": account.sec_balance,
                        "balance": account.balance,
                    }));
                };
            }
            print_json(&rows)
        }
    }
}
