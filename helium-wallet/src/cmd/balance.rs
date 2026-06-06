use crate::{cmd::*, contacts};
use helium_lib::{
    keypair::Pubkey,
    token::{self, Token},
};

#[derive(Debug, clap::Args)]
/// Get the balance for a wallet or a given public key. The balance is given for
/// each of the Helium related holdings of a given Solana address. The
/// address may be a base58 Solana pubkey or a contact name.
pub struct Cmd {
    #[arg(value_parser = contacts::parse_address_or_name)]
    address: Option<Pubkey>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let address = opts.maybe_wallet_key(self.address)?;
        let client = opts.client()?;
        let balances =
            token::balance_for_addresses(&client, &Token::associated_token_addresses(&address))
                .await?;
        let mut json = json!({
            "address": address.to_string(),
            "balance": token::TokenBalanceMap::from(balances),
        });
        if let Some(contact) = contacts::cached().find_by_address(&address) {
            json["name"] = json!(contact.name);
        }
        print_json(&json)
    }
}
