use crate::cmd::*;
use helium_lib::{dao::SubDao, entity_key::EntityKeyEncoding, reward};

#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: RewardsCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum RewardsCommand {
    Pending(PendingCmd),
    Claim(ClaimCmd),
}

impl RewardsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Pending(cmd) => cmd.run(opts).await,
            Self::Claim(cmd) => cmd.run(opts).await,
        }
    }
}

#[derive(Clone, Debug, clap::Args)]
/// List pending rewards for given hotspots
pub struct PendingCmd {
    /// Subdao for command
    subdao: SubDao,

    /// Hotspot public keys to look up
    hotspots: Vec<helium_crypto::PublicKey>,
}

impl PendingCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let settings: Settings = opts.try_into()?;
        let entity_key_strings = hotspots_to_entity_key_strings(&self.hotspots);
        let pending = reward::pending(
            &settings,
            &self.subdao,
            &entity_key_strings,
            EntityKeyEncoding::UTF8,
        )
        .await?;

        print_json(&pending)
    }
}

#[derive(Clone, Debug, clap::Args)]
/// Claim rewards for one or all hotspots in a wallet
pub struct ClaimCmd {
    /// Subdao for command
    subdao: SubDao,

    /// Hotspot public keys to claim rewawrds for
    hotspots: Vec<helium_crypto::PublicKey>,
    #[command(flatten)]
    commit: CommitOpts,
}

impl ClaimCmd {
    pub async fn run(&self, _opts: Opts) -> Result {
        // let password = get_wallet_password(false)?;
        // let wallet = load_wallet(&opts.files)?;
        // let keypair = wallet.decrypt(password.as_bytes())?;
        // let settings = opts.try_into()?;
        // let entity_key = hotspot::key_to_entity(&self.hotspot)?;
        // asset::get_bulk_rewards(&settings, &SubDao::Iot, &entity_key)?;
        unimplemented!();
        // Ok(())
    }
}

fn hotspots_to_entity_key_strings(public_keys: &[helium_crypto::PublicKey]) -> Vec<String> {
    public_keys
        .iter()
        .map(|key| key.to_string())
        .collect::<Vec<String>>()
}
