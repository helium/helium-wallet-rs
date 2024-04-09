use crate::cmd::*;
use helium_lib::{
    asset,
    dao::SubDao,
    entity_key::{self, EntityKeyEncoding},
    reward,
};

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
    Current(CurrentCmd),
    Pending(PendingCmd),
    Init(InitCmd),
}

impl RewardsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Current(cmd) => cmd.run(opts).await,
            Self::Pending(cmd) => cmd.run(opts).await,
            Self::Init(cmd) => cmd.run(opts).await,
        }
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List current (totel lifetime) rewards issued for a given entity key
pub struct CurrentCmd {
    /// Subdao for command
    subdao: SubDao,

    /// Entity key to look up
    entity_key: String,
}

impl CurrentCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let settings: Settings = opts.try_into()?;
        let current = reward::current(&settings, &self.subdao, &self.entity_key).await?;

        print_json(&current)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List claimable pending rewards for a given asset
pub struct PendingCmd {
    /// Subdao for command
    subdao: SubDao,

    /// Entity key to look up
    entity_key: String,
}

impl PendingCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let settings: Settings = opts.try_into()?;
        let pending = reward::pending(
            &settings,
            &self.subdao,
            &[self.entity_key.clone()],
            EntityKeyEncoding::String,
        )
        .await?;

        print_json(&pending)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// Inititialize reward recipient
pub struct InitCmd {
    /// Subdao for command
    subdao: SubDao,

    /// Entity key to init the rewards recipient for
    entity_key: String,

    #[command(flatten)]
    commit: CommitOpts,
}

impl InitCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let settings: Settings = opts.try_into()?;

        init(
            &settings,
            keypair.clone(),
            &self.commit,
            &self.subdao,
            &self.entity_key,
            EntityKeyEncoding::String,
        )
        .await
    }
}

pub async fn init(
    settings: &Settings,
    keypair: Arc<Keypair>,
    commit: &CommitOpts,
    subdao: &SubDao,
    entity_key_string: &str,
    entity_key_encoding: EntityKeyEncoding,
) -> Result {
    let client = settings.mk_anchor_client(Keypair::void())?;
    let entity_key = entity_key::from_string(entity_key_string.to_string(), entity_key_encoding)?;
    let asset_account = asset::account_for_entity_key(&client, &entity_key).await?;
    let json = match reward::recipient::for_asset_account(&client, subdao, &asset_account).await {
        Ok(Some(_)) => json!({"result": "ok"}),
        Ok(None) => {
            let txn =
                reward::recipient::init(settings, subdao, &entity_key, keypair.clone()).await?;
            commit.maybe_commit(&txn, settings).await?.to_json()
        }
        Err(err) => return Err(Error::from(err)),
    };
    print_json(&json)
}
