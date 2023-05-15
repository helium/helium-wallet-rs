use crate::{cmd::*, result::Result, token::Token};
use serde_json::json;

#[derive(Clone, Debug, clap::Args)]
/// Get the current HNT price for Data Credits
pub struct Cmd {
    /// The USD value for amount of DC that is intended to be created.
    usd: f64,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let client = new_client(&opts.url)?;
        let price = client.get_pyth_price(Token::Hnt)?;
        let json = json!({
                "usd": self.usd,
                "dc": (self.usd * 100_000_000.0).abs() as u64,
                "price": {
                    "hnt": price,
                }
        });
        print_json(&json)
    }
}
