use crate::{
    cmd::{squads as cmd_squads, *},
    contacts,
};
use helium_lib::{asset, dao, entity_key, keypair::Pubkey};

#[derive(Clone, Debug, clap::Args)]
/// Burn a given asset (NFT)
pub struct Cmd {
    /// Subdao for command
    subdao: dao::SubDao,
    /// Entity key of asset to burn
    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
    /// Submit as a Squads v4 proposal — see `transfer one --squads`.
    /// The asset's current owner must be the resolved vault.
    #[arg(long, value_parser = contacts::parse_address_or_name)]
    squads: Option<Pubkey>,
    /// Memo recorded on the v4 proposal (`--squads` only).
    #[arg(long)]
    memo: Option<String>,
    /// Commit the transaction
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let signer = opts.load_signer()?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let asset = asset::for_entity_key(&client, &self.entity_key.as_entity_key()?).await?;

        if let Some(squads_target) = self.squads {
            let client_ref = &client;
            let asset_id = asset.id;
            return cmd_squads::submit_proposal_with(
                client_ref,
                squads_target,
                self.memo.clone(),
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

        let (tx, _) = asset::burn(&client, &asset.id, &*signer, &transaction_opts).await?;

        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
