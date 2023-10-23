use crate::{cmd::*, keypair::Pubkey, result::Result};

#[derive(Clone, Debug, clap::Args)]
/// Get the list of hotspots for the active or a given wallet
pub struct Cmd {
    /// The alternate wallet to get the list of hotspots for
    wallet: Option<Pubkey>,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let owner = if let Some(walet) = self.wallet {
            walet
        } else {
            let wallet = load_wallet(&opts.files)?;
            wallet.public_key
        };
        let client = new_client(&opts.url)?;
        let hotspots = client.get_hotspots(&owner)?;
        let json = json!( {
            "address": owner.to_string(),
            "hotspots": hotspots,
        });
        print_json(&json)
    }
}
