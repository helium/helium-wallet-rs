use crate::{cmd::*, keypair::Pubkey, result::Result};

#[derive(Debug, clap::Args)]
/// Get the balance for a wallet or a given public key. The balance is given for
/// each of the Helium related holdings of a given Solana address
pub struct Cmd {
    address: Option<Pubkey>,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let address = if let Some(address) = self.address {
            address
        } else {
            let wallet = load_wallet(&opts.files)?;
            wallet.public_key
        };

        let client = new_client(&opts.url)?;

        let balances = client.get_balances(&address)?;
        let json = json!({
            "address": address.to_string(),
            "balance": balances,
        });
        print_json(&json)
    }
}
