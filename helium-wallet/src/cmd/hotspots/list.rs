use crate::cmd::*;
use helium_lib::{hotspot, keypair::Pubkey};

#[derive(Clone, Debug, clap::Args)]
/// Get the list of hotspots for the active or a given wallet
pub struct Cmd {
    /// The alternate wallet to get the list of hotspots for
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
        let settings = opts.try_into()?;
        let hotspots = hotspot::for_owner(&settings, &owner).await?;
        let json = json!( {
            "address": owner.to_string(),
            "hotspots": hotspots,
        });
        print_json(&json)
    }
}
