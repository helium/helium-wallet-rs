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
        let owner = opts.maybe_wallet_key(self.wallet)?;
        let client = opts.client()?;
        let hotspots = hotspot::for_owner(&client, &owner).await?;
        let json = json!( {
            "address": owner.to_string(),
            "hotspots": hotspots,
        });
        print_json(&json)
    }
}
