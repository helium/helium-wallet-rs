use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::SwapInstructionsRequest,
    keypair::Signer,
    token::{Token, TokenAmount},
};

/// Default slippage tolerance in basis points (100 = 1%).
const DEFAULT_SLIPPAGE_BPS: u16 = 100;

#[derive(Debug, Clone, clap::Args)]
/// Swap tokens via the blockchain-api (Jupiter-backed)
pub struct Cmd {
    /// Input token (hnt, mobile, iot, usdc, sol)
    input_token: Token,
    /// Output token (hnt, mobile, iot, usdc, sol)
    output_token: Token,
    /// Amount to swap (human-readable, e.g. 1.5 for 1.5 HNT)
    amount: f64,
    /// Slippage tolerance in basis points (100 = 1%)
    #[arg(long, default_value_t = DEFAULT_SLIPPAGE_BPS)]
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

        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;

        let input_mint = self.input_token.mint();
        let output_mint = self.output_token.mint();
        let raw_amount = TokenAmount::from_f64(self.input_token, self.amount)?.amount;

        // Quote first: it drives the cost display and is passed back verbatim
        // to build the swap transaction.
        let quote = api
            .swap_quote(input_mint, output_mint, raw_amount, self.slippage_bps)
            .await?;
        let response = api
            .swap_instructions(&SwapInstructionsRequest {
                quote_response: quote.clone(),
                user_public_key: signer.pubkey().to_string(),
                destination_token_account: None,
            })
            .await?;

        let committed = self
            .commit
            .commit_via_api(&api, &client, &response, &*signer)
            .await?;

        let mut json = committed.to_json();
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
