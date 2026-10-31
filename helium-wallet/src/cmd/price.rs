use crate::cmd::*;
use helium_lib::token;

#[derive(Clone, Debug, clap::Args)]
/// Get the HNT price from the oracle the data-credits program pins
pub struct Cmd {
    /// Token to look up (HNT only)
    #[arg(value_parser = token::Token::hnt_value_parser)]
    token: token::Token,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let price = token::price::get(&client, self.token).await?;

        print_json(&price)
    }
}
