use crate::cmd::*;
use anyhow::Context;
use helium_lib::{dao::SubDao, entity_key, kta, reward, token::TokenAmount};

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
    Claim(ClaimCmd),
    Pending(PendingCmd),
    MaxClaim(MaxClaimCmd),
}

impl RewardsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Claim(cmd) => cmd.run(opts).await,
            Self::MaxClaim(cmd) => cmd.run(opts).await,
            Self::Pending(cmd) => cmd.run(opts).await,
        }
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List current (totel lifetime) rewards issued for a given entity key
pub struct ClaimCmd {
    /// Subdao for command
    subdao: SubDao,
    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
    /// The optional amount to claim
    ///
    /// If not specific the full pending amount is claimed, limited by the maximum
    /// claim amount for the subdao
    amount: Option<f64>,
    /// Do not check and initialize the on chain recipient
    ///
    /// For known assets that have been previously initialized this will speed up the claim
    #[arg(long)]
    skip_init: bool,
    /// Commit the claim transaction.
    #[command(flatten)]
    commit: CommitOpts,
}

impl ClaimCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;
        let hotspot_kta = kta::for_entity_key(&self.entity_key.as_entity_key()?).await?;
        let transaction_opts = self.commit.transaction_opts();

        let mut init_response = None;
        if !self.skip_init {
            let recipient = reward::recipient::for_kta(&client, &self.subdao, &hotspot_kta).await?;
            if recipient.is_none() {
                let (tx, _) = reward::recipient::init(
                    &client,
                    &self.subdao,
                    &self.entity_key.as_entity_key()?,
                    &keypair,
                    &transaction_opts,
                )
                .await?;
                init_response = Some(
                    self.commit
                        .maybe_commit(&tx, &client)
                        .await
                        .context("while initializing reward recipient")?,
                );
            }
        }

        let token_amount = self
            .amount
            .map(|amount| TokenAmount::from_f64(self.subdao.token(), amount).amount);
        let Some((tx, _)) = reward::claim(
            &client,
            &self.subdao,
            token_amount,
            &self.entity_key,
            &keypair,
            &transaction_opts,
        )
        .await?
        else {
            bail!("No rewards to claim")
        };

        let claim_response = self
            .commit
            .maybe_commit(&tx, &client)
            .await
            .context("while claiming rewards")?;
        let json = if let Some(init_response) = init_response {
            json!({
                "init": init_response,
                "claim": claim_response,
            })
        } else {
            claim_response.to_json()
        };

        print_json(&json)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List the maximum claim amount for the given subdao
///
/// The max claim amoount is the configured threshold for the subdao, adjusted down by a time
/// decayed amount bed on previous claims
pub struct MaxClaimCmd {
    /// Subdao for command
    subdao: SubDao,
}

impl MaxClaimCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let max_claim = reward::max_claim(&client, &self.subdao).await?;

        print_json(&max_claim)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List claimable pending rewards for a given asset
pub struct PendingCmd {
    /// Subdao for command
    subdao: SubDao,

    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
}

impl PendingCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let pending = reward::pending(
            &client,
            &self.subdao,
            &[self.entity_key.entity_key.clone()],
            self.entity_key.encoding.into(),
        )
        .await?;

        print_json(&pending)
    }
}
