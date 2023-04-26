use crate::{cmd::*, result::Result};

#[derive(Clone, Debug, clap::Args)]
/// Get the list of hotspots for this wallet
pub struct Cmd {}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let hotspots = client.get_hotspots(&wallet.public_key)?;
        let json = json!( {
            "address": wallet.public_key.to_string(),
            "hotspots": hotspots,
        });
        print_json(&json)
    }
}
