use crate::cmd::*;
use helium_lib::{dao::SubDao, hotspot};

#[derive(Clone, Debug, clap::Args)]
/// Get details for a given Hotspot
pub struct Cmd {
    address: helium_crypto::PublicKey,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let hotspot = hotspot::get_with_info(&client, &SubDao::all(), &self.address).await?;
        print_json(&hotspot)
    }
}
