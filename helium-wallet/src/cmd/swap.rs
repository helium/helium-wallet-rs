use crate::cmd::*;
use futures::TryFutureExt;
use helium_lib::{jupiter, token::Token};

#[derive(Debug, Clone, clap::Args)]
/// Swap tokens via Jupiter
pub struct Cmd {
    /// Input token (hnt, mobile, iot, usdc, sol)
    input_token: Token,
    /// Output token (hnt, mobile, iot, usdc, sol)
    output_token: Token,
    /// Amount to swap (human-readable, e.g. 1.5 for 1.5 HNT)
    amount: f64,
    /// Slippage tolerance in basis points (100 = 1%)
    #[arg(long, default_value_t = jupiter::DEFAULT_SLIPPAGE_BPS)]
    slippage_bps: u16,
    /// Commit the swap
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;

        let jupiter_client = jupiter::Client::from_env().map_err(|e| anyhow!("Jupiter: {e}"))?;

        let input_mint = self.input_token.mint();
        let output_mint = self.output_token.mint();
        let raw_amount =
            helium_lib::token::TokenAmount::from_f64(self.input_token, self.amount).amount;

        let quote = jupiter_client
            .quote(input_mint, output_mint, raw_amount)
            .map_err(|e| anyhow!("Quote failed: {e}"))
            .await?;

        let (tx, _) = jupiter_client.swap(&client, &quote, &keypair).await?;

        let response = self.commit.maybe_commit(tx, &client).await?;
        let mut json = response.to_json();
        if let serde_json::Value::Object(ref mut map) = json {
            map.insert("in_amount".to_string(), quote.in_amount.into());
            map.insert("out_amount".to_string(), quote.out_amount.into());
            map.insert("input_mint".to_string(), quote.input_mint.into());
            map.insert("output_mint".to_string(), quote.output_mint.into());
            map.insert("slippage_bps".to_string(), quote.slippage_bps.into());
            map.insert(
                "price_impact_pct".to_string(),
                quote.price_impact_pct.into(),
            );
        }
        print_json(&json)
    }
}
