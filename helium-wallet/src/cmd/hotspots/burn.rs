use crate::cmd::{squads as cmd_squads, *};
use helium_lib::{dao, hotspot, keypair::Pubkey};

#[derive(Clone, Debug, clap::Args)]
/// Burn a given Hotspot NFT
pub struct Cmd {
    /// Subdao for command
    subdao: dao::SubDao,
    /// Key for the Hotspot NFT to burn
    address: helium_crypto::PublicKey,
    /// Submit as a Squads v4 proposal — see `transfer one --squads`.
    /// The hotspot's current owner must be the resolved vault.
    #[arg(long)]
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
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let transaction_opts = self.commit.transaction_opts(&client);

        if let Some(squads_target) = self.squads {
            let client_ref = &client;
            let address = self.address.clone();
            return cmd_squads::submit_proposal_with(
                client_ref,
                squads_target,
                self.memo.clone(),
                &keypair,
                &self.commit,
                &transaction_opts,
                |vault| async move {
                    Ok(vec![
                        hotspot::fetch_burn_instruction(client_ref, &address, vault.as_pubkey())
                            .await?,
                    ])
                },
            )
            .await;
        }

        let (tx, _) = hotspot::burn(&client, &self.address, &keypair, &transaction_opts).await?;

        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
