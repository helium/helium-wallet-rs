use crate::result::Result;
use helium_api::{Account, Client};
use prettytable::{format, Table};

pub fn cmd_balance(addresses: Vec<String>) -> Result {
    let client = Client::new();
    let mut results = Vec::with_capacity(addresses.len());
    for address in addresses {
        results.push((address.to_string(), client.get_account(&address)));
    }
    print_results(results);
    Ok(())
}

fn print_results(results: Vec<(String, Result<Account>)>) {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
    table.set_titles(row![
        "Address",
        "Balance",
        "Data Credits",
        "Security Tokens"
    ]);

    for (address, result) in results {
        match result {
            Ok(account) => table.add_row(row![
                address,
                account.balance,
                account.dc_balance,
                account.security_balance
            ]),
            Err(err) => table.add_row(row![address, H3 -> err.to_string()]),
        };
    }
    table.printstd();
}
