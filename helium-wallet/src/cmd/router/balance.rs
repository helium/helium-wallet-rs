use crate::cmd::*;
use helium_lib::{dao::SubDao, token};

#[derive(Debug, clap::Args)]
/// Get the Delegated DC balance for a given router key. The balance is in Data
/// Credits (DC)
pub struct Cmd {
    #[arg(value_enum)]
    pub subdao: SubDao,
    pub router_key: String,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let delegated_dc_key = self.subdao.delegated_dc_key(&self.router_key);
        let escrow_key = self.subdao.escrow_key(&delegated_dc_key);
        let client = opts.client()?;
        let balance = token::balance_for_address(&client, &escrow_key)
            .await?
            .or(Some(token::Token::Dc.to_balance(escrow_key, 0)))
            .map(|balance| balance.amount);
        let json = json!({
            "router": self.router_key,
            "delegated_dc_key": delegated_dc_key.to_string(),
            "escrow_key": escrow_key.to_string(),
            "balance": balance.unwrap_or_default(),
        });
        print_json(&json)
    }
}
