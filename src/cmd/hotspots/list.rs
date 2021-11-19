use crate::{
    cmd::*,
    keypair::PublicKey,
    result::{anyhow, Result},
};
use helium_api::{accounts, models::Hotspot, IntoVec};
use prettytable::{format, Table};

#[derive(Debug, StructOpt)]
/// Get the list of hotspots for one or more wallet addresses
pub struct Cmd {
    /// Addresses to get hotspots for
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
        let mut results: Vec<(PublicKey, Result<Vec<Hotspot>>)> =
            Vec::with_capacity(self.addresses.len());
        for address in addresses {
            let hotspots: Result<Vec<Hotspot>> = accounts::hotspots(&client, &address.to_string())
                .into_vec()
                .await
                .map_err(|e| e.into());
            results.push((address.clone(), hotspots));
        }
        print_results(results, opts.format)
    }
}

fn print_results(results: Vec<(PublicKey, Result<Vec<Hotspot>>)>, format: OutputFormat) -> Result {
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
            table.set_titles(row!["Address", "Name", "Location", "City", "State"]);

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
                                hotspot.location.unwrap_or_else(|| "unknown".to_string()),
                                hotspot
                                    .geocode
                                    .short_city
                                    .unwrap_or_else(|| "unknown".to_string()),
                                hotspot
                                    .geocode
                                    .short_state
                                    .unwrap_or_else(|| "unknown".to_string())
                            ]);
                        }
                    }
                    Err(err) => {
                        table.add_row(row![address.to_string(), H5 -> err.to_string()]);
                    }
                };
            }
            print_table(&table)
        }
        OutputFormat::Json => {
            let mut table = Vec::with_capacity(results.len());
            for (address, result) in results {
                let mut table_hotspots = vec![];
                if let Ok(hotspots) = result {
                    for hotspot in hotspots {
                        table_hotspots.push(json!({
                            "address": hotspot.address,
                            "name":  hotspot.name.unwrap_or_else(|| "unknown".to_string()),
                            "location": hotspot.location.unwrap_or_else(|| "unknown".to_string()),
                            "city":
                                hotspot
                                    .geocode
                                    .short_city
                                .unwrap_or_else(|| "unknown".to_string()),
                            "state":
                                hotspot
                                    .geocode
                                    .short_state
                                .unwrap_or_else(|| "unknown".to_string())
                        }))
                    }
                };
                table.push(json!({
                    "address": address.to_string(),
                    "hotspots": table_hotspots,
                }));
            }
            print_json(&table)
        }
    }
}
