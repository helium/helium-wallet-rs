use crate::cmd::{
    squads::{self as cmd_squads, SquadsOpts},
    *,
};
use helium_lib::{
    blockchain_api::types::TransferHotspotRequest,
    entity_key,
    keypair::{Pubkey, Signer},
};

#[derive(Clone, Debug, clap::Args)]
/// Transfer an asset (NFT) to another owner
pub struct Cmd {
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,

    /// Solana address of the recipient of the asset
    pub recipient: Pubkey,
    /// Submit as a Squads v4 proposal.
    /// The asset's current owner must be the resolved vault.
    #[command(flatten)]
    pub squads: SquadsOpts,
    /// Commit the transfer
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;

        // With `--squads` the transfer is built from the multisig's vault (which
        // must own the asset) and wrapped as a proposal; otherwise it transfers
        // from this wallet. Both build via the API.
        let (multisig, memo) = match self.squads.squads {
            Some(target) => (
                Some(cmd_squads::resolve_multisig(&client, target).await?),
                self.squads.memo.clone(),
            ),
            None => (None, None),
        };

        let api = opts.blockchain_api()?;
        let response = api
            .transfer_hotspot(&TransferHotspotRequest {
                wallet_address: signer.pubkey().to_string(),
                hotspot_pubkey: self.entity_key.to_string(),
                recipient: self.recipient.to_string(),
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
