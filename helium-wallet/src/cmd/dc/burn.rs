use crate::cmd::{squads as cmd_squads, *};
use helium_lib::{dao, dc, keypair::Pubkey};

#[derive(Debug, Clone, clap::Args)]
/// Burn Data Credits (DC) from this wallet or delegated for the given router into oblivion.
pub struct Cmd {
    /// Amount of DC to burn
    dc: u64,
    /// Subdao to use for delegated burn
    subdao: Option<dao::SubDao>,
    /// Router key to burn on behalf of
    ///
    /// Note that the wallet keypair must be the burn authority for the router key
    /// for the burn to succeed
    router: Option<String>,
    /// Submit as a Squads v4 proposal — see `transfer one --squads`.
    /// Only supported for the non-delegated (no router) burn path; the
    /// DC is sourced from the resolved vault's DC ATA.
    #[arg(long)]
    squads: Option<Pubkey>,
    /// Memo recorded on the v4 proposal (`--squads` only).
    #[arg(long)]
    memo: Option<String>,
    /// Commit the burn
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
            if self.router.is_some() || self.subdao.is_some() {
                bail!("--squads only supports the non-delegated burn path");
            }
            return cmd_squads::submit_proposal_with(
                &client,
                squads_target,
                self.memo.clone(),
                &keypair,
                &self.commit,
                &transaction_opts,
                |vault| async move { Ok(vec![dc::burn_instruction(self.dc, vault.as_pubkey())]) },
            )
            .await;
        }

        let (tx, _) = match (&self.router, self.subdao) {
            (Some(router_key), Some(subdao)) => {
                dc::burn_delegated(
                    &client,
                    subdao,
                    &keypair,
                    self.dc,
                    router_key,
                    &transaction_opts,
                )
                .await?
            }
            (None, None) => dc::burn(&client, self.dc, &keypair, &transaction_opts).await?,
            _ => bail!("both router and subdao must be specified"),
        };
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
