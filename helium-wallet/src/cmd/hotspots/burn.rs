use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::entity_key::EncodedEntityKey;

#[derive(Clone, Debug, clap::Args)]
/// Burn a given Hotspot NFT
pub struct Cmd {
    /// Key for the Hotspot NFT to burn
    address: helium_crypto::PublicKey,
    /// Submit as a Squads v4 proposal.
    /// The hotspot's current owner must be the resolved vault.
    #[command(flatten)]
    squads: SquadsOpts,
    /// Commit the transaction
    #[command(flatten)]
    commit: CommitOpts,
}

impl From<&Cmd> for crate::cmd::assets::burn::Cmd {
    fn from(value: &Cmd) -> Self {
        Self {
            entity_key: EncodedEntityKey::from(&value.address),
            squads: value.squads.clone(),
            commit: value.commit.clone(),
        }
    }
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        crate::cmd::assets::burn::Cmd::from(self).run(opts).await
    }
}
