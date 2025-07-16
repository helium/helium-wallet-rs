use crate::cmd::*;
use helium_lib::{
    keypair::Pubkey,
    token::{self, Token},
};

#[derive(Debug, clap::Args)]
/// Get the balance for a wallet or a given public key. The balance is given for
/// each of the Helium related holdings of a given Solana address
pub struct Cmd {
    address: Option<Pubkey>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let address = opts.maybe_wallet_key(self.address)?;
        let client = opts.client()?;
        let balances =
            token::balance_for_addresses(&client, &Token::associated_token_adresses(&address))
                .await?;
        let json = json!({
            "address": address.to_string(),
            "balance": token::TokenBalanceMap::from(balances),
        });
        print_json(&json)
    }
}
