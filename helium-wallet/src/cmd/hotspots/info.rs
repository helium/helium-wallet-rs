use crate::cmd::*;
use helium_lib::{dao::SubDao, hotspot};

#[derive(Clone, Debug, clap::Args)]
/// Get details for a given hotspot
pub struct Cmd {
    address: helium_crypto::PublicKey,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let settings = opts.try_into()?;
        let hotspot = hotspot::get_with_info(&settings, &SubDao::all(), &self.address).await?;
        print_json(&hotspot)
    }
}
