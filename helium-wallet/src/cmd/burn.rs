use crate::cmd::{
    squads::{self as cmd_squads, SquadsOpts},
    *,
};
use helium_lib::{
    blockchain_api::types::{TokenAmountInput, TokenBurnRequest},
    dao::SubDao,
    keypair::Signer,
    token,
};

#[derive(Debug, Clone, clap::Args)]
/// Burn tokens
pub struct Cmd {
    /// Subdao token to burn
    subdao: SubDao,
    /// Amount to burn
    amount: f64,
    #[command(flatten)]
    squads: SquadsOpts,
    /// Commit the burn
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;

        let token_amount = token::TokenAmount::from_f64(self.subdao.token(), self.amount)?;

        if let Some(squads_target) = self.squads.squads {
            let txn_opts = self.commit.transaction_opts(&client);
            return cmd_squads::submit_proposal_with(
                &client,
                squads_target,
                self.squads.memo.clone(),
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

        let api = opts.blockchain_api()?;
        let response = api
            .token_burn(&TokenBurnRequest {
                wallet_address: signer.pubkey().to_string(),
                token_amount: TokenAmountInput::new(token_amount.token.mint(), token_amount.amount),
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}
