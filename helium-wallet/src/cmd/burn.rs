use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::{
    blockchain_api::types::{TokenAmountInput, TokenBurnRequest},
    dao::SubDao,
    keypair::Signer,
    programs::KnownProgram,
    token,
    transaction::VersionedTransaction,
    verify,
};

/// A proposal burns the vault's tokens inside a Squads instruction this cannot
/// read, so it is held to moving nothing at the top level: without that, a
/// plain burn returned in place of a proposal would destroy this wallet's own
/// balance unverified.
fn assert_wraps_no_burn(unsigned: &[VersionedTransaction]) -> Result<()> {
    let compute_budget = KnownProgram::ComputeBudget.id();
    let spl_token = KnownProgram::SplToken.id();
    for tx in unsigned {
        for ix in tx.message.instructions() {
            let program = verify::instruction_program(tx, ix)?;
            if program != compute_budget && program == spl_token {
                bail!(
                    "a Squads proposal must not burn at the top level, but this transaction \
                     invokes {program}; refusing to sign"
                );
            }
        }
    }
    Ok(())
}

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

        // With `--squads`, the burn is built from the resolved vault and wrapped
        // as a proposal by the API; otherwise it burns from the wallet directly.
        let (multisig, memo) = self.squads.resolve(&client, &signer.pubkey()).await?;
        let is_proposal = multisig.is_some();

        let api = opts.blockchain_api()?;
        let response = api
            .token_burn(&TokenBurnRequest {
                wallet_address: signer.pubkey().to_string(),
                token_amount: TokenAmountInput::new(token_amount.token.mint(), token_amount.amount),
                multisig,
                memo,
            })
            .await?;
        let unsigned = response.decode_transactions()?;
        if is_proposal {
            assert_wraps_no_burn(&unsigned)?;
        } else {
            verify::assert_spl_burn(
                &unsigned,
                &signer.pubkey(),
                token_amount.token,
                token_amount.amount,
            )?;
        }
        print_json(
            &self
                .commit
                .commit_via_api(
                    &api,
                    &client,
                    &response,
                    &*signer,
                    ApiSigning::FreshBlockhash,
                )
                .await?
                .to_json(),
        )
    }
}
