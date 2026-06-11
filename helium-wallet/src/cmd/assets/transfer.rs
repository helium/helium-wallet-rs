use crate::cmd::{
    squads::{self as cmd_squads, SquadsOpts},
    *,
};
use helium_lib::{
    asset, entity_key,
    keypair::{Pubkey, Signer},
    kta,
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
        let transaction_opts = self.commit.transaction_opts(&client);
        let kta = kta::for_entity_key(&self.entity_key.as_entity_key()?).await?;

        if let Some(squads_target) = self.squads.squads {
            let client_ref = &client;
            let recipient = self.recipient;
            let asset_id = kta.asset;
            return cmd_squads::submit_proposal_with(
                client_ref,
                squads_target,
                self.squads.memo.clone(),
                &*signer,
                &self.commit,
                &transaction_opts,
                |vault| async move {
                    if vault.as_pubkey() == &recipient {
                        bail!("recipient already owner of asset");
                    }
                    Ok(vec![
                        asset::fetch_transfer_instruction(
                            client_ref,
                            &asset_id,
                            &recipient,
                            vault.as_pubkey(),
                        )
                        .await?,
                    ])
                },
            )
            .await;
        }

        if signer.pubkey() == self.recipient {
            bail!("recipient already owner of asset");
        }
        let (tx, _) = asset::transfer(
            &client,
            &kta.asset,
            &self.recipient,
            &*signer,
            &transaction_opts,
        )
        .await?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
