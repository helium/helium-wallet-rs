use crate::cmd::*;
use helium_lib::{dao::SubDao, dc, keypair::Keypair, solana_sdk};

#[derive(Debug, Clone, clap::Args)]
/// Burn 1 Data Credit (DC) from another wallet into oblivion.
pub struct Cmd {
    /// Subdao to burn delegated DC to
    subdao: SubDao,

    /// Router key to derive escrow account that DC will be burned from.
    router_key: String,

    /// DC burn authority keypair
    keypair: PathBuf,

    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let sol_keypair = solana_sdk::signature::read_keypair_file(&self.keypair)
            .map_err(|_err| anyhow::anyhow!("could not read keypair file"))?;
        let keypair = Keypair::from(sol_keypair);

        let client = opts.client()?;
        let transanction_opts = self.commit.transaction_opts(&client);

        let (tx, _) = dc::burn_delegated(
            &client,
            self.subdao,
            &keypair,
            1, // burning 1 DC
            &self.router_key,
            &transanction_opts,
        )
        .await?;

        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
