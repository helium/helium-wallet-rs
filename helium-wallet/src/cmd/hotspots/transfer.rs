use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::{entity_key::EncodedEntityKey, keypair::Pubkey};

#[derive(Clone, Debug, clap::Args)]
/// Transfer a Hotspot to another owner
pub struct Cmd {
    /// Key of Hotspot
    address: helium_crypto::PublicKey,
    /// Solana address of Recipient of Hotspot
    recipient: Pubkey,
    /// Submit as a Squads v4 proposal.
    /// The hotspot's current owner must be the resolved vault.
    #[command(flatten)]
    squads: SquadsOpts,
    /// Commit the transfer
    #[command(flatten)]
    commit: CommitOpts,
}

impl From<&Cmd> for crate::cmd::assets::transfer::Cmd {
    fn from(value: &Cmd) -> Self {
        Self {
            entity_key: EncodedEntityKey::from(&value.address),
            recipient: value.recipient,
            squads: value.squads.clone(),
            commit: value.commit.clone(),
        }
    }
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        crate::cmd::assets::transfer::Cmd::from(self)
            .run(opts)
            .await
    }
}
