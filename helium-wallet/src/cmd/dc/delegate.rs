use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::{blockchain_api::types::DcDelegateRequest, dao::SubDao, keypair::Signer};

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

        // In propose mode the memo rides on the Squads proposal; a direct
        // delegate sends no in-tx memo (matching prior behavior).
        let (multisig, memo) = self.squads.resolve(&client, &signer.pubkey()).await?;

        let api = opts.blockchain_api()?;
        let response = api
            .dc_delegate(&DcDelegateRequest {
                owner: signer.pubkey().to_string(),
                router_key: self.payer.clone(),
                amount: self.dc.to_string(),
                mint: self.subdao.token().mint().to_string(),
                memo,
                multisig,
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
