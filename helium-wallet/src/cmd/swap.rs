use crate::cmd::*;
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
    /// Least output to accept (human-readable, in the output token). The
    /// swap is refused unless the order guarantees at least this much.
    #[arg(long)]
    min_output: f64,
    /// Slippage tolerance in basis points (100 = 1%)
    #[arg(long, default_value_t = jupiter::DEFAULT_SLIPPAGE_BPS)]
    slippage_bps: u16,
    /// Commit the swap
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        if self.amount <= 0.0 || !self.amount.is_finite() {
            bail!("swap amount must be a positive finite number");
        }
        if self.min_output <= 0.0 || !self.min_output.is_finite() {
            bail!("minimum output must be a positive finite number");
        }

        let signer = opts.load_signer()?;
        let client = opts.client()?;

        let jupiter_client = jupiter::Client::from_env()?;

        let input_mint = self.input_token.mint();
        let output_mint = self.output_token.mint();
        let raw_amount =
            helium_lib::token::TokenAmount::from_f64(self.input_token, self.amount)?.amount;
        let raw_min_output =
            helium_lib::token::TokenAmount::from_f64(self.output_token, self.min_output)?.amount;

        let (tx, _, order) = jupiter_client
            .swap(
                &client,
                input_mint,
                output_mint,
                raw_amount,
                raw_min_output,
                &*signer,
            )
            .await?;

        let response = self.commit.maybe_commit(tx, &client).await?;
        let mut json = response.to_json();
        if let serde_json::Value::Object(ref mut map) = json {
            map.insert("in_amount".to_string(), order.in_amount.into());
            map.insert("out_amount".to_string(), order.out_amount.into());
            map.insert("input_mint".to_string(), order.input_mint.into());
            map.insert("output_mint".to_string(), order.output_mint.into());
            map.insert("slippage_bps".to_string(), order.slippage_bps.into());
            map.insert(
                "price_impact_pct".to_string(),
                order.price_impact_pct.into(),
            );
        }
        print_json(&json)
    }
}
