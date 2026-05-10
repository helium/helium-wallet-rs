use crate::cmd::{squads as cmd_squads, *};
use helium_lib::{dao::SubDao, dc, keypair::Pubkey};

#[derive(Debug, Clone, clap::Args)]
/// Delegate DC from this wallet to a given router
pub struct Cmd {
    /// Subdao to delegate DC to
    subdao: SubDao,

    /// Public Helium payer key to delegate to
    payer: String,

    /// Amount of DC to delegate
    dc: u64,

    /// Submit as a Squads v4 proposal — see `transfer one --squads`.
    /// The DC is sourced from the resolved vault's DC ATA.
    #[arg(long)]
    squads: Option<Pubkey>,
    /// Memo recorded on the v4 proposal (`--squads` only).
    #[arg(long)]
    memo: Option<String>,

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

        if let Some(squads_target) = self.squads {
            return cmd_squads::submit_proposal_with(
                &client,
                squads_target,
                self.memo.clone(),
                &keypair,
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
            &keypair,
            &transaction_opts,
        )
        .await?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
