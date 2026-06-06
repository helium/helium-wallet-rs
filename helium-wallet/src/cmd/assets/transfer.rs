use crate::{
    cmd::{squads as cmd_squads, *},
    contacts,
};
use helium_lib::{
    asset, entity_key,
    keypair::{Pubkey, Signer},
    kta,
};

#[derive(Clone, Debug, clap::Args)]
/// Transfer a Hotspot to another owner
pub struct Cmd {
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,

    /// Recipient of the Hotspot — base58 Solana pubkey or contact name.
    #[arg(value_parser = contacts::parse_address_or_name)]
    recipient: Pubkey,
    /// Submit as a Squads v4 proposal — see `transfer one --squads`.
    /// The asset's current owner must be the resolved vault.
    #[arg(long, value_parser = contacts::parse_address_or_name)]
    squads: Option<Pubkey>,
    /// Memo recorded on the v4 proposal (`--squads` only).
    #[arg(long)]
    memo: Option<String>,
    /// Commit the transfer
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let kta = kta::for_entity_key(&self.entity_key.as_entity_key()?).await?;

        if let Some(squads_target) = self.squads {
            let client_ref = &client;
            let recipient = self.recipient;
            let asset_id = kta.asset;
            return cmd_squads::submit_proposal_with(
                client_ref,
                squads_target,
                self.memo.clone(),
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
            bail!("recipient already owner of hotspot");
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
