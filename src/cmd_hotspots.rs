use crate::result::Result;
use helium_api::{Client, Hotspot};
use prettytable::{format, Table};

pub fn cmd_hotspots(url: String, addresses: Vec<String>) -> Result {
    let client = Client::new_with_base_url(url);
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
    table.set_titles(row!["Address", "Name", "Location", "City", "State", "Score"]);

    for (address, result) in results {
        #[allow(clippy::unused_unit)]
        match result {
            Ok(hotspots) if hotspots.is_empty() => {
                table.add_row(row![address, H5 -> "No hotspots found".to_string()]);
                ()
            }
            Ok(hotspots) => {
                for hotspot in hotspots {
                    table.add_row(row![
                        hotspot.address,
                        hotspot.name.unwrap_or_else(|| "unknown".to_string()),
                        hotspot.location.unwrap_or_else(|| "uknown".to_string()),
                        hotspot.geocode.short_city.unwrap_or_else(|| "uknown".to_string()),
                        hotspot.geocode.short_state.unwrap_or_else(|| "uknown".to_string()),
                        hotspot.score
                    ]);
                }
            }
            Err(err) => {
                table.add_row(row![address, H5 -> err.to_string()]);
                ()
            }
        };
    }
    table.printstd();
}
