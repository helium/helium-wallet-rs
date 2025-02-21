use std::time::Instant;

use crate::cmd::*;
use anyhow::Context;
use helium_lib::{dao::SubDao, dc};

#[derive(Debug, Clone, clap::Args)]
/// Burn 1 Data Credit (DC) from another wallet into oblivion.
pub struct Cmd {
    /// Subdao to burn delegated DC to
    subdao: SubDao,

    /// Router key to derive escrow account that DC will be burned from.
    router_key: String,

    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;

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
