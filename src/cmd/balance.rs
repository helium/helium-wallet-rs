use crate::{client::to_token_balance_map, cmd::*, keypair::Pubkey, result::Result};
use hpl_utils::token::Token;

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

        let balances =
            client.get_balance_for_addresses(&Token::associated_token_adresses(&address))?;
        let json = json!({
            "address": address.to_string(),
            "balance": to_token_balance_map(balances),
        });
        print_json(&json)
    }
}
