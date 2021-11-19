use crate::{
    cmd::*,
    keypair::PublicKey,
    result::{anyhow, Result},
};
use helium_api::{accounts, models::Validator, IntoVec};
use prettytable::{format, Table};

#[derive(Debug, StructOpt)]
/// Get the list of validators owned by one or more wallet addresses
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
        let mut results: Vec<(PublicKey, Result<Vec<Validator>>)> =
            Vec::with_capacity(self.addresses.len());
        for address in addresses {
            let validators: Result<Vec<Validator>> =
                accounts::validators(&client, &address.to_string())
                    .into_vec()
                    .await
                    .map_err(|e| e.into());
            results.push((address.clone(), validators));
        }
        print_results(results, opts.format)
    }
}

fn print_results(
    results: Vec<(PublicKey, Result<Vec<Validator>>)>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
            table.set_titles(row!["Address", "Owner", "Stake", "Status"]);

            for (address, result) in results {
                #[allow(clippy::unused_unit)]
                match result {
                    Ok(validators) if validators.is_empty() => {
                        table.add_row(row![address, H4 -> "No validators found".to_string()]);
                        ()
                    }
                    Ok(validators) => {
                        for validator in validators {
                            table.add_row(row![
                                validator.address,
                                validator.owner,
                                validator.stake,
                                validator.stake_status
                            ]);
                        }
                    }
                    Err(err) => {
                        table.add_row(row![address.to_string(), H4 -> err.to_string()]);
                    }
                };
            }
            print_table(&table)
        }
        OutputFormat::Json => {
            let mut table = Vec::with_capacity(results.len());
            for (address, result) in results {
                let mut table_validators = vec![];
                if let Ok(validators) = result {
                    for validator in validators {
                        table_validators.push(json!({
                            "address": validator.address,
                            "owner": validator.owner,
                            "last_heartbeat": validator.last_heartbeat,
                            "version_heartbeat": validator.version_heartbeat,
                            "stake": validator.stake,
                            "status": validator.stake_status,
                        }))
                    }
                };
                table.push(json!({
                    "address": address.to_string(),
                    "validators": table_validators,
                }));
            }
            print_json(&table)
        }
    }
}
