use crate::{cmd::*, dao::SubDao, hotspot, result::Result};

#[derive(Clone, Debug, clap::Args)]
/// Get details for a given hotspot
pub struct Cmd {
    ecc_key: helium_crypto::PublicKey,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let settings = opts.try_into()?;
        let info = hotspot::get_info(&settings, &SubDao::all(), &self.ecc_key)?;
        print_json(&info)
    }
}
