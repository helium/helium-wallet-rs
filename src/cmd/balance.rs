use crate::{cmd::*, result::Result};

#[derive(Debug, clap::Args)]
/// Get the balance for a wallet. The balance is given for each of the Helium
/// related holdings of a given Solana address
pub struct Cmd {}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;

        let balances = client.get_balances(&wallet.public_key)?;
        let json = json!({
            "address": wallet.public_key.to_string(),
            "balance": balances,
        });
        print_json(&json)
    }
}
