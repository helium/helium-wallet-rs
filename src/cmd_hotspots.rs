use crate::result::Result;
use helium_api::{Client, Hotspot};
use prettytable::{format, Table};

pub fn cmd_hotspots(addresses: Vec<String>) -> Result {
    let client = Client::default();
    let mut results: Vec<(String, Result<Vec<Hotspot>>)> = Vec::with_capacity(addresses.len());
    for address in addresses {
        results.push((address.to_string(), client.get_hotspots(&address)));
    }
    print_results(results);
    Ok(())
}

fn print_results(results: Vec<(String, Result<Vec<Hotspot>>)>) {
    let mut table = Table::new();
    table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
    table.set_titles(row!["Address", "Name", "City", "State", "Score"]);

    for (address, result) in results {
        #[allow(clippy::unused_unit)]
        match result {
            Ok(hotspots) => {
                for hotspot in hotspots {
                    table.add_row(row![
                        hotspot.address,
                        hotspot.name.unwrap_or_else(|| "uknown".to_string()),
                        hotspot.short_city,
                        hotspot.short_state,
                        hotspot.score
                    ]);
                }
            }
            Err(err) => {
                table.add_row(row![address, H4 -> err.to_string()]);
                ()
            }
        };
    }
    table.printstd();
}
