use crate::cmd::*;
use helium_lib::{asset, dao, entity_key};

#[derive(Clone, Debug, clap::Args)]
/// Burn a given asset (NFT)
pub struct Cmd {
    /// Subdao for command
    subdao: dao::SubDao,
    /// Entity key of asset to burn
    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
    /// Commit the transaction
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let asset = asset::for_entity_key(&client, &self.entity_key.as_entity_key()?).await?;
        let tx = asset::burn(
            &client,
            &asset.id,
            &keypair,
            &self.commit.transaction_opts(),
        )
        .await?;

        print_json(&self.commit.maybe_commit(&tx, &client).await?.to_json())
    }
}
