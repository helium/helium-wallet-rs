use crate::cmd::*;
use helium_lib::token::{self, Token};
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
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
        let settings = opts.try_into()?;
        let price = token::pyth_price(&settings, Token::Hnt).await?;
        let decimals = price.expo.unsigned_abs();

        // Remove the confidence from the price to use the most conservative price
        // https://docs.pyth.network/pythnet-price-feeds/best-practices
        let hnt_price = Decimal::new(price.price, decimals)
            - (Decimal::new(price.conf as i64, decimals) * dec!(2));

        let usd_amount =
            Decimal::from_f64(self.usd).ok_or_else(|| anyhow!("Invalid USD amount"))?;
        let dc_amount = (usd_amount * dec!(100_000))
            .to_u64()
            .ok_or_else(|| anyhow!("Invalid USD amount"))?;
        let hnt_amount = (usd_amount / hnt_price).round_dp(Token::Hnt.decimals().into());

        let json = json!({
                "usd": self.usd,
                "hnt": hnt_amount,
                "dc": dc_amount,
                "hnt_price": hnt_price,
        });
        print_json(&json)
    }
}
