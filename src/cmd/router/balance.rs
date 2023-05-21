use crate::{cmd::*, dao::SubDao, result::Result, token::Token};

#[derive(Debug, clap::Args)]
/// Get the Delegated DC balance for a given router key. The balance is in Data
/// Credits (DC)
pub struct Cmd {
    #[arg(value_enum)]
    pub subdao: SubDao,
    pub router_key: String,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let client = new_client(&opts.url)?;
        let delegated_dc_key = self.subdao.delegated_dc_key(&self.router_key);
        let mut balances = client.get_balances(&delegated_dc_key)?;
        let json = json!({
            "router": self.router_key,
            "delegated_dc_key": delegated_dc_key.to_string(),
            "balance": {
                "dc": balances.remove(&Token::Dc).map(|balance| balance.amount).unwrap_or_default(),
            }
        });
        print_json(&json)
    }
}
