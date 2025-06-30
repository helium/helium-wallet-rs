use crate::cmd::*;
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

    /// Solana address of Recipient of Hotspot
    recipient: Pubkey,
    /// Commit the transfer
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        if keypair.pubkey() == self.recipient {
            bail!("recipient already owner of hotspot");
        }
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let kta = kta::for_entity_key(&self.entity_key.as_entity_key()?).await?;
        let (tx, _) = asset::transfer(
            &client,
            &kta.asset,
            &self.recipient,
            &keypair,
            &transaction_opts,
        )
        .await?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
