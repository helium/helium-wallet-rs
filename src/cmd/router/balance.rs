use crate::{cmd::*, dao::SubDao, result::Result, token};

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
        let settings = opts.try_into()?;
        let delegated_dc_key = self.subdao.delegated_dc_key(&self.router_key);
        let escrow_key = self.subdao.escrow_account_key(&delegated_dc_key);
        let balance =
            token::get_balance_for_address(&settings, &escrow_key)?.map(|balance| balance.amount);
        let json = json!({
            "router": self.router_key,
            "delegated_dc_key": delegated_dc_key.to_string(),
            "escrow_key": escrow_key.to_string(),
            "balance": {
                "dc": balance.unwrap_or_default(),
            }
        });
        print_json(&json)
    }
}
