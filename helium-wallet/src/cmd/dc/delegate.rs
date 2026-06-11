use crate::cmd::{
    squads::{self as cmd_squads, SquadsOpts},
    *,
};
use helium_lib::{dao::SubDao, dc};

#[derive(Debug, Clone, clap::Args)]
/// Delegate DC from this wallet to a given router
pub struct Cmd {
    /// Subdao to delegate DC to
    subdao: SubDao,

    /// Public Helium payer key to delegate to
    payer: String,

    /// Amount of DC to delegate
    dc: u64,

    /// The DC is sourced from the resolved vault's DC ATA.
    #[command(flatten)]
    squads: SquadsOpts,

    /// Commit the delegation
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;

        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);

        if let Some(squads_target) = self.squads.squads {
            return cmd_squads::submit_proposal_with(
                &client,
                squads_target,
                self.squads.memo.clone(),
                &*signer,
                &self.commit,
                &transaction_opts,
                |vault| async move {
                    Ok(vec![dc::delegate_instruction(
                        self.subdao,
                        &self.payer,
                        self.dc,
                        vault.as_pubkey(),
                    )])
                },
            )
            .await;
        }

        let (tx, _) = dc::delegate(
            &client,
            self.subdao,
            &self.payer,
            self.dc,
            &*signer,
            &transaction_opts,
        )
        .await?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
