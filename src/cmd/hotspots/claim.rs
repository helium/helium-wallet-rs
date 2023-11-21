use crate::{cmd::*, result::Result};

#[derive(Clone, Debug, clap::Args)]
/// Claim rewards for one or all hotspots in a wallet
pub struct Cmd {
    /// A hotspot public key to claim rewawrds for
    hotspot: Option<helium_crypto::PublicKey>,
}

impl Cmd {
    pub fn run(&self, _opts: Opts) -> Result {
        // let password = get_wallet_password(false)?;
        // let wallet = load_wallet(&opts.files)?;
        // let keypair = wallet.decrypt(password.as_bytes())?;
        Ok(())
    }
}
