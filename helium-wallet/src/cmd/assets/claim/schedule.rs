use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::{
        AddEntityToAutomationRequest, AddWalletToAutomationRequest, CloseAutomationRequest,
        FundAutomationRequest, RemoveEntityFromAutomationRequest, RequeueAutomationRequest,
        SetupAutomationRequest,
    },
    entity_key,
    keypair::{Pubkey, Signer},
    queue, schedule, token,
};

#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: Command,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

/// Schedule repeating claims for a wallet using Tuktuk
///
/// A wallet has a single claim cron. Set it up with `init`, attach claims to it
/// with `wallet` (every hotspot the wallet owns) or `one` (a single hotspot),
/// and keep it funded with `fund`. All transactions are built by the
/// blockchain-api and signed locally.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {
    Init(InitCmd),
    Fund(FundCmd),
    Wallet(WalletCmd),
    One(OneCmd),
    Remove(RemoveCmd),
    Requeue(RequeueCmd),
    Close(CloseCmd),
    Info(InfoCmd),
}

impl Command {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Init(cmd) => cmd.run(opts).await,
            Self::Fund(cmd) => cmd.run(opts).await,
            Self::Wallet(cmd) => cmd.run(opts).await,
            Self::One(cmd) => cmd.run(opts).await,
            Self::Remove(cmd) => cmd.run(opts).await,
            Self::Requeue(cmd) => cmd.run(opts).await,
            Self::Close(cmd) => cmd.run(opts).await,
            Self::Info(cmd) => cmd.run(opts).await,
        }
    }
}

/// Set up the claim cron for this wallet
///
/// Creates the cron on the given schedule and pre-funds it for `--duration`
/// claim cycles. Attach claims afterwards with `schedule wallet` or
/// `schedule one`, and add more funding with `schedule fund`.
#[derive(Clone, Debug, clap::Args)]
pub struct InitCmd {
    /// The schedule to claim on, as a crontab string.
    ///
    /// The schedule is specified in an [enhanced crontab format](https://github.com/clockwork-xyz/clockwork/blob/main/cron/README.md#%EF%B8%8F-syntax),
    /// which requires at least one more field than the basic crontab format.
    ///
    /// For example:
    /// // sec  min   hour   day of month   month   day of week   year
    /// "   0     0     0        1            *         *           *
    ///
    /// runs at midnight on the first day of every month.
    schedule: String,
    /// Number of claim cycles to pre-fund.
    #[arg(long, default_value_t = 30)]
    duration: u32,
    #[command(flatten)]
    commit: CommitOpts,
}

impl InitCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .setup_automation(&SetupAutomationRequest {
                wallet_address: signer.pubkey().to_string(),
                cron_schedule: self.schedule.clone(),
                duration: self.duration,
                // Required by the API; the server recomputes the real hotspot
                // count when sizing the funding.
                total_hotspots: 1,
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}

/// Add funding for more claim cycles to this wallet's cron
///
/// Funds both pools (the cron-crank pool and the claim-payer pool) for
/// `--duration` additional claim cycles.
#[derive(Clone, Debug, clap::Args)]
pub struct FundCmd {
    /// Number of additional claim cycles to fund.
    #[arg(long)]
    duration: u32,
    #[command(flatten)]
    commit: CommitOpts,
}

impl FundCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .fund_automation(&FundAutomationRequest {
                wallet_address: signer.pubkey().to_string(),
                additional_duration: self.duration,
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}

/// Add a whole-wallet claim to this wallet's cron
///
/// The cron claims every hotspot the wallet owns each time it fires.
#[derive(Clone, Debug, clap::Args)]
pub struct WalletCmd {
    #[command(flatten)]
    commit: CommitOpts,
}

impl WalletCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .add_wallet_to_automation(&AddWalletToAutomationRequest {
                wallet_address: signer.pubkey().to_string(),
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}

/// Add a single hotspot's claim to this wallet's cron
#[derive(Clone, Debug, clap::Args)]
pub struct OneCmd {
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    #[command(flatten)]
    commit: CommitOpts,
}

impl OneCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .add_entity_to_automation(&AddEntityToAutomationRequest {
                wallet_address: signer.pubkey().to_string(),
                entity_key: self.entity_key.to_string(),
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}

/// Remove a single claim entry from this wallet's cron
///
/// Use `schedule info` to see how many claim entries the cron has; entries are
/// indexed from 0. Removing an entry refunds its rent.
#[derive(Clone, Debug, clap::Args)]
pub struct RemoveCmd {
    /// Index of the claim entry to remove.
    #[arg(long)]
    index: u32,
    #[command(flatten)]
    commit: CommitOpts,
}

impl RemoveCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .remove_entity_from_automation(&RemoveEntityFromAutomationRequest {
                wallet_address: signer.pubkey().to_string(),
                index: self.index,
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}

/// Requeue this wallet's cron after it ran out of SOL
///
/// Fund the cron first (`schedule fund`) so it stays queued.
#[derive(Clone, Debug, clap::Args)]
pub struct RequeueCmd {
    #[command(flatten)]
    commit: CommitOpts,
}

impl RequeueCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .requeue_automation(&RequeueAutomationRequest {
                wallet_address: signer.pubkey().to_string(),
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}

/// Remove this wallet's cron
///
/// Removes all claim entries and closes the cron, refunding rent.
#[derive(Clone, Debug, clap::Args)]
pub struct CloseCmd {
    #[command(flatten)]
    commit: CommitOpts,
}

impl CloseCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .close_automation(&CloseAutomationRequest {
                wallet_address: signer.pubkey().to_string(),
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}

/// Display information about the cron for this wallet
///
/// Shows the schedule, the number of claim entries, and the balances of the two
/// pools that keep it running: the cron-crank wallet and the claim wallet.
#[derive(Clone, Debug, clap::Args)]
pub struct InfoCmd {
    /// The wallet to look up claim information for.
    /// Defaults to the active wallet.
    pub wallet: Option<Pubkey>,
}

impl InfoCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        #[derive(Debug, serde::Serialize, Default)]
        struct CronJobInfo {
            schedule: String,
            cron_jobs: u32,
            cron_wallet: token::TokenBalance,
            claim_wallet: token::TokenBalance,
        }

        let wallet = opts.maybe_wallet_key(self.wallet)?;
        let client = opts.client()?;

        let cron_job_key = schedule::cron_job_key_for_wallet(&wallet, 0);
        let claim_wallet = queue::claim_wallet_key(&queue::TASK_QUEUE_ID, &wallet);

        let mut info = CronJobInfo {
            claim_wallet: token::balance_for_address(&client, &claim_wallet)
                .await?
                .unwrap_or(token::Token::Sol.to_balance(claim_wallet, 0)),
            cron_wallet: token::balance_for_address(&client, &cron_job_key)
                .await?
                .unwrap_or(token::Token::Sol.to_balance(cron_job_key, 0)),
            ..Default::default()
        };
        if let Some(cronjob) = schedule::get(&client, &cron_job_key).await? {
            info.schedule = cronjob.schedule;
            info.cron_jobs = cronjob.num_transactions;
        };

        print_json(&info)
    }
}
