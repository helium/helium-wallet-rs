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
            .await
            .map_err(|e| anyhow!("Quote failed: {e}"))?;

        let in_amount = helium_lib::token::TokenAmount::from_u64(
            self.input_token,
            quote.in_amount.parse().unwrap_or(0),
        );
        let out_amount = helium_lib::token::TokenAmount::from_u64(
            self.output_token,
            quote.out_amount.parse().unwrap_or(0),
        );
        let in_f64 = f64::from(&in_amount);
        let out_f64 = f64::from(&out_amount);
        let input_token = self.input_token;
        let output_token = self.output_token;
        let slippage_bps = quote.slippage_bps;
        let price_impact_pct = &quote.price_impact_pct;

        eprintln!("Swap: {in_f64} {input_token} → ~{out_f64} {output_token} (slippage: {slippage_bps}bps, impact: {price_impact_pct}%)");

        let (tx, _) = jupiter_client.swap(&client, &quote, &keypair).await?;

        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
