use crate::cmd::{
    squads::{self as cmd_squads, SquadsOpts},
    *,
};
use helium_lib::{asset, blockchain_api::types::HotspotBurnRequest, entity_key, keypair::Signer};

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

        if let Some(squads_target) = self.squads.squads {
            let transaction_opts = self.commit.transaction_opts(&client);
            let asset = asset::for_entity_key(&client, &self.entity_key.as_entity_key()?).await?;
            let client_ref = &client;
            let asset_id = asset.id;
            return cmd_squads::submit_proposal_with(
                client_ref,
                squads_target,
                self.squads.memo.clone(),
                &*signer,
                &self.commit,
                &transaction_opts,
                |vault| async move {
                    Ok(vec![
                        asset::fetch_burn_instruction(client_ref, &asset_id, vault.as_pubkey())
                            .await?,
                    ])
                },
            )
            .await;
        }

        let api = opts.blockchain_api()?;
        let response = api
            .burn_hotspot(&HotspotBurnRequest {
                wallet_address: signer.pubkey().to_string(),
                hotspot_pubkey: self.entity_key.to_string(),
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
