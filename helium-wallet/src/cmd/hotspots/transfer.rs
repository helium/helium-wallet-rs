use crate::cmd::*;
use helium_lib::{
    hotspot,
    keypair::{GetPubkey, Pubkey},
};

#[derive(Clone, Debug, clap::Args)]
/// Transfer a hotspot to another owner
pub struct Cmd {
    /// Key of hotspot
    address: helium_crypto::PublicKey,
    /// Solana address of Recipient of hotspot
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
        let settings = opts.clone().try_into()?;
        let tx = hotspot::transfer(&settings, &self.address, &self.recipient, keypair).await?;
        print_json(&self.commit.maybe_commit(&tx, &settings).await?.to_json())
    }
}
