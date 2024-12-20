use crate::cmd::*;
use helium_lib::{hotspot, keypair::Pubkey};

#[derive(Clone, Debug, clap::Args)]
/// Get the list of Hotspots for the active or a given wallet
pub struct Cmd {
    /// The alternate wallet to get the list of Hotspots for
    wallet: Option<Pubkey>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let owner = if let Some(walet) = self.wallet {
            walet
        } else {
            let wallet = opts.load_wallet()?;
            wallet.public_key
        };
        let client = opts.client()?;
        let hotspots = hotspot::for_owner(&client, &owner).await?;
        let json = json!( {
            "address": owner.to_string(),
            "hotspots": hotspots,
        });
        print_json(&json)
    }
}
