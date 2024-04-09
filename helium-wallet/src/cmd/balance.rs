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
        let address = if let Some(address) = self.address {
            address
        } else {
            let wallet = opts.load_wallet()?;
            wallet.public_key
        };
        let settings = opts.try_into()?;

        let balances =
            token::balance_for_addresses(&settings, &Token::associated_token_adresses(&address))
                .await?;
        let json = json!({
            "address": address.to_string(),
            "balance": token::TokenBalanceMap::from(balances),
        });
        print_json(&json)
    }
}
