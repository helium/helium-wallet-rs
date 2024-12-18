use crate::cmd::*;
use helium_lib::{dao, hotspot};

#[derive(Clone, Debug, clap::Args)]
/// Burn a given Hotspot NFT
pub struct Cmd {
    /// Subdao for command
    subdao: dao::SubDao,
    /// Key for the Hotspot NFT to burn
    address: helium_crypto::PublicKey,
    /// Commit the transaction
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let tx = hotspot::burn(
            &client,
            &self.address,
            &keypair,
            &self.commit.transaction_opts(),
        )
        .await?;

        print_json(&self.commit.maybe_commit(&tx, &client).await?.to_json())
    }
}
