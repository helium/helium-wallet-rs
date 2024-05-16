use crate::cmd::*;
use helium_lib::{dao::SubDao, hotspot, keypair::Signature};

#[derive(Clone, Debug, clap::Args)]
/// Get metadata updates for a given hotspot
///
/// NOTE: Hotspots that were onboarded before the Solana transition will
/// not include the metadata at transition time as part of the update list.
pub struct Cmd {
    /// Subdao to fetch updates for
    subdao: SubDao,
    /// The hotspot to fetch udpates for
    address: helium_crypto::PublicKey,
    /// The signature to start looking backwards from
    #[arg(long)]
    before: Option<Signature>,
    /// The signathre to look backwards up to
    #[arg(long)]
    until: Option<Signature>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let settings = opts.try_into()?;
        let params = hotspot::info::HotspotInfoUpdateParams {
            before: self.before,
            until: self.until,
            ..Default::default()
        };
        let txns = hotspot::info::updates(&settings, self.subdao, &self.address, params).await?;
        print_json(&txns)
    }
}
