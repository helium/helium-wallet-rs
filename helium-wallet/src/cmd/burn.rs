use crate::{
    cmd::{squads as cmd_squads, *},
    contacts,
};
use helium_lib::{dao::SubDao, keypair::Pubkey, token};

#[derive(Debug, Clone, clap::Args)]
/// Burn tokens
pub struct Cmd {
    /// Subdao token to burn
    subdao: SubDao,
    /// Amount to burn
    amount: f64,
    /// Submit as a Squads v4 proposal — see `transfer one --squads`.
    #[arg(long, value_parser = contacts::parse_address_or_name)]
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
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let txn_opts = self.commit.transaction_opts(&client);

        let token_amount = token::TokenAmount::from_f64(self.subdao.token(), self.amount);

        if let Some(squads_target) = self.squads {
            return cmd_squads::submit_proposal_with(
                &client,
                squads_target,
                self.memo.clone(),
                &*signer,
                &self.commit,
                &txn_opts,
                |vault| async move {
                    Ok(vec![token::burn_instruction(
                        vault.as_pubkey(),
                        &token_amount,
                    )?])
                },
            )
            .await;
        }

        let (tx, _) = token::burn(&client, &token_amount, &*signer, &txn_opts).await?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
