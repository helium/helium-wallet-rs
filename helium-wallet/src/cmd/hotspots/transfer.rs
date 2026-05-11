use crate::cmd::{squads as cmd_squads, *};
use helium_lib::{
    hotspot,
    keypair::{Pubkey, Signer},
};

#[derive(Clone, Debug, clap::Args)]
/// Transfer a Hotspot to another owner
pub struct Cmd {
    /// Key of Hotspot
    address: helium_crypto::PublicKey,
    /// Solana address of Recipient of Hotspot
    recipient: Pubkey,
    /// Submit as a Squads v4 proposal — see `transfer one --squads`.
    /// The hotspot's current owner must be the resolved vault.
    #[arg(long)]
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
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);

        if let Some(squads_target) = self.squads {
            let client_ref = &client;
            let address = self.address.clone();
            let recipient = self.recipient;
            return cmd_squads::submit_proposal_with(
                client_ref,
                squads_target,
                self.memo.clone(),
                &keypair,
                &self.commit,
                &transaction_opts,
                |vault| async move {
                    if vault.as_pubkey() == &recipient {
                        bail!("recipient already owner of hotspot");
                    }
                    Ok(vec![
                        hotspot::fetch_transfer_instruction(
                            client_ref,
                            &address,
                            &recipient,
                            vault.as_pubkey(),
                        )
                        .await?,
                    ])
                },
            )
            .await;
        }

        if keypair.pubkey() == self.recipient {
            bail!("recipient already owner of hotspot");
        }
        let (tx, _) = hotspot::transfer(
            &client,
            &self.address,
            &self.recipient,
            &keypair,
            &transaction_opts,
        )
        .await?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
