use crate::{cmd::*, contacts};
use helium_lib::{hotspot, keypair::Pubkey};

#[derive(Clone, Debug, clap::Args)]
/// Get the list of Hotspots for the active or a given wallet. The
/// wallet may be a base58 Solana pubkey or a contact name.
pub struct Cmd {
    /// The alternate wallet to get the list of Hotspots for
    #[arg(value_parser = contacts::parse_address_or_name)]
    wallet: Option<Pubkey>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let owner = opts.maybe_wallet_key(self.wallet)?;
        let client = opts.client()?;
        let hotspots = hotspot::for_owner(&client, &owner).await?;
        let mut json = json!( {
            "address": owner.to_string(),
            "hotspots": hotspots,
        });
        if let Some(contact) = contacts::cached().find_by_address(&owner) {
            json["name"] = json!(contact.name);
        }
        print_json(&json)
    }
}
