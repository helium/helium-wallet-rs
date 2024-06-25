use crate::cmd::*;
use helium_lib::token::{self, Token};
use rust_decimal::prelude::*;
use serde_json::json;

#[derive(Clone, Debug, clap::Args)]
/// Get the amount of HNT needed to buy a given number of USD worth of Data
/// Credits
pub struct Cmd {
    /// The USD value of the Data Credits to convert to HNT amount.
    usd: f64,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let price = token::price::get(&client, Token::Hnt).await?;

        let hnt_price = price.price;
        let usd_amount =
            Decimal::from_f64(self.usd).ok_or_else(|| anyhow!("Invalid USD amount"))?;
        let dc_amount = (usd_amount * Decimal::new(token::price::DC_PER_USD, 0))
            .to_u64()
            .ok_or_else(|| anyhow!("Invalid USD amount"))?;
        let hnt_amount = (usd_amount / hnt_price).round_dp(Token::Hnt.decimals().into());

        let json = json!({
                "usd": self.usd,
                "hnt": hnt_amount,
                "dc": dc_amount,
                "hnt_price": hnt_price,
                "timestamp": price.timestamp,
        });
        print_json(&json)
    }
}
