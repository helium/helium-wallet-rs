use crate::cmd::*;
use helium_lib::token;

#[derive(Clone, Debug, clap::Args)]
/// Get the current price from the pyth price feed for the given token
pub struct Cmd {
    /// Token to look up
    #[arg(value_parser = token::Token::pricekey_value_parser)]
    token: token::Token,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let settings = opts.try_into()?;
        let price = token::price::get(&settings, self.token).await?;

        print_json(&price)
    }
}
