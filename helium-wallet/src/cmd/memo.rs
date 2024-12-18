use crate::cmd::*;

/// Send a memo to the blockchain
#[derive(Debug, clap::Args)]
pub struct Cmd {
    /// message to send.
    ///
    /// Remain under about 500 bytes for the message
    message: String,
    /// Commit the message
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = opts.load_wallet()?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts();
        let tx =
            helium_lib::memo::memo(&client, &self.message, &keypair, &transaction_opts).await?;
        print_json(&self.commit.maybe_commit(&tx, &client).await?.to_json())
    }
}
