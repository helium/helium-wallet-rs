use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::{blockchain_api::types::DcBurnRequest, keypair::Signer};

#[derive(Debug, Clone, clap::Args)]
/// Burn Data Credits (DC) from this wallet into oblivion.
pub struct Cmd {
    /// Amount of DC to burn
    dc: u64,
    /// Burn from the resolved vault's DC ATA as a Squads v4 proposal instead of
    /// from this wallet.
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

        // With `--squads` the API burns from the resolved vault as a proposal;
        // otherwise it burns directly from this wallet.
        let (multisig, memo) = self.squads.resolve(&client, &signer.pubkey()).await?;

        let api = opts.blockchain_api()?;
        let response = api
            .dc_burn(&DcBurnRequest {
                owner: signer.pubkey().to_string(),
                amount: self.dc.to_string(),
                multisig,
                memo,
            })
            .await?;
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
