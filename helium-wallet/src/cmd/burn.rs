use crate::cmd::*;
use helium_lib::{dao::SubDao, token};

#[derive(Debug, Clone, clap::Args)]
/// Burn tokens
pub struct Cmd {
    /// Subdao token to burn
    subdao: SubDao,
    /// Amount to burn
    amount: f64,
    /// Commit the burn
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;

        let token_amount = token::TokenAmount::from_f64(self.subdao.token(), self.amount);
        let tx = token::burn(&client, &token_amount, &keypair).await?;
        print_json(&self.commit.maybe_commit(&tx, &client).await?.to_json())
    }
}
