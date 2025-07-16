use crate::cmd::*;
use helium_lib::{asset, entity_key, keypair};

#[derive(Clone, Debug, clap::Args)]
/// Get details for a given asset
pub struct Cmd {
    /// Display raw asset data
    #[arg(long)]
    raw: bool,
    /// Entity key of asset to look up
    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let asset = asset::for_entity_key(&client, &self.entity_key.as_entity_key()?).await?;
        if self.raw {
            print_json(&asset)
        } else {
            print_json(&AssetInfo::from(asset))
        }
    }
}

#[derive(serde::Serialize)]
struct AssetInfo {
    #[serde(with = "keypair::serde_pubkey")]
    id: keypair::Pubkey,
    #[serde(with = "keypair::serde_pubkey")]
    owner: keypair::Pubkey,
    symbol: String,
    name: String,
}

impl From<asset::Asset> for AssetInfo {
    fn from(value: asset::Asset) -> Self {
        Self {
            id: value.id,
            owner: value.ownership.owner,
            symbol: value.content.metadata.symbol,
            name: value.content.metadata.name,
        }
    }
}
