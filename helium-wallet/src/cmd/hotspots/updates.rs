use crate::cmd::*;
use helium_lib::{dao::SubDao, hotspot, keypair::Signature};

#[derive(Clone, Debug, clap::Args)]
/// Get metadata updates for a given Hotspot
///
/// NOTE: Hotspots that were onboarded before the Solana transition will
/// not include the metadata at transition time as part of the update list.
pub struct Cmd {
    /// Subdao to fetch updates for
    subdao: SubDao,
    /// The Hotspot to fetch updates for
    address: helium_crypto::PublicKey,
    /// The signature to start looking backwards from
    #[arg(long)]
    before: Option<Signature>,
    /// The signature to look backwards up to
    #[arg(long)]
    until: Option<Signature>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let params = hotspot::info::HotspotInfoUpdateParams {
            before: self.before,
            until: self.until,
            ..Default::default()
        };
        let info_key = self.subdao.info_key(&self.address);
        let txns = hotspot::info::updates(&client, &info_key, params).await?;
        print_json(&txns)
    }
}
