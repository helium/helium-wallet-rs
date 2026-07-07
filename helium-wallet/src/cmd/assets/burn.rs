use crate::cmd::{
    squads::{self as cmd_squads, SquadsOpts},
    *,
};
use helium_lib::{blockchain_api::types::HotspotBurnRequest, entity_key, keypair::Signer};

#[derive(Clone, Debug, clap::Args)]
/// Burn a given asset (NFT)
pub struct Cmd {
    /// Entity key of asset to burn
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    /// Submit as a Squads v4 proposal.
    /// The asset's current owner must be the resolved vault.
    #[command(flatten)]
    pub squads: SquadsOpts,
    /// Commit the transaction
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let signer = opts.load_signer()?;

        // With `--squads` the API builds the burn from the multisig's vault
        // (which must own the asset) and wraps it as a proposal; otherwise the
        // wallet burns its own asset directly.
        let (multisig, memo) = match self.squads.squads {
            Some(squads_target) => (
                Some(cmd_squads::resolve_multisig(&client, squads_target).await?),
                self.squads.memo.clone(),
            ),
            None => (None, None),
        };

        let api = opts.blockchain_api()?;
        let response = api
            .burn_hotspot(&HotspotBurnRequest {
                wallet_address: signer.pubkey().to_string(),
                hotspot_pubkey: self.entity_key.to_string(),
                multisig,
                memo,
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
