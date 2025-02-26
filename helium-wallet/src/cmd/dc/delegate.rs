use crate::cmd::*;
use helium_lib::{dao::SubDao, dc};

#[derive(Debug, Clone, clap::Args)]
/// Delegate DC from this wallet to a given router
pub struct Cmd {
    /// Subdao to delegate DC to
    subdao: SubDao,

    /// Public Helium payer key to delegate to
    payer: String,

    /// Amount of DC to delgate
    dc: u64,

    /// Commit the delegation
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;

        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let (tx, _) = dc::delegate(
            &client,
            self.subdao,
            &self.payer,
            self.dc,
            &keypair,
            &transaction_opts,
        )
        .await?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
