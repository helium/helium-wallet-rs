use crate::cmd::*;
use helium_lib::{
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

/// Sechedule a repeating claim transactions with Tuktuk
#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {
    Init(InitCmd),
    Info(InfoCmd),
    Close(CloseCmd),
    Wallet(ClaimWalletCmd),
}

impl Command {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Wallet(cmd) => cmd.run(opts).await,
            Self::Init(cmd) => cmd.run(opts).await,
            Self::Info(cmd) => cmd.run(opts).await,
            Self::Close(cmd) => cmd.run(opts).await,
        }
    }
}

const SCHEDULE_NAME: &str = "entity_claim";

/// Initializes a repeating schedule for this wallet using Tuktuk
///
/// To reduce complexity, only one schedule is supported for a wallet, but multiple
/// jobs can run within the initialized schedule. Using `schedule wallet_claim`, for example,
/// multiple wallet claims can be scheduled on the active schedule.
///
/// Note that the resulting schedule needs SOL to keep the schedule running. Check the balance
/// with the `claim schedule info` command.  
#[derive(Clone, Debug, clap::Args)]
pub struct InitCmd {
    /// The schedule to claim on
    ///
    /// The schedule is specified in an [enhanced crontab format](https://github.com/clockwork-xyz/clockwork/blob/main/cron/README.md#%EF%B8%8F-syntax). Note that the specification requires at least one more field
    /// than the basic crontab format.
    ///
    /// For example:
    /// // sec  min   hour   day of month   month   day of week   year
    /// "   0     0     0        1            *         *           *
    ///
    /// Will initialize a schedule that runs at midnight on the first day of every month
    schedule: String,
    /// Optionally fund the initialized schedule with the given amount of SOL
    #[arg(long)]
    fund: Option<f64>,
    #[command(flatten)]
    commit: CommitOpts,
}

impl InitCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = opts.maybe_wallet_key(None)?;
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);

        let cronjob_key = schedule::cron_job_key_for_wallet(&wallet, 0);
        if let Some(cronjob) = schedule::get(&client, &cronjob_key).await? {
            if cronjob.schedule != self.schedule || cronjob.name != SCHEDULE_NAME {
                bail!(
                    "A different schedule \"{}\" with name \"{}\" already exists for this wallet",
                    cronjob.schedule,
                    cronjob.name
                );
            }
            return print_json(&json!({ "result": "ok"}));
        }

        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let fund = self
            .fund
            .map(|amount| token::TokenAmount::from_f64(token::Token::Sol, amount).amount);
        let (tx, _) = schedule::init(
            &client,
            &queue::TASK_QUEUE_ID,
            0,
            (&self.schedule, SCHEDULE_NAME),
            fund,
            &keypair,
            &transaction_opts,
        )
        .await?;

        print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
    }
}

/// Displays information about the schedule for this wallet
///
/// This includes the schedule, name and current balance of the wallet funding the schedule itself,
/// separate from the wallets for each of the jobs that run within the schedule.
#[derive(Clone, Debug, clap::Args)]
pub struct InfoCmd {}

impl InfoCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        #[derive(Debug, serde::Serialize)]
        struct CronJobInfo {
            schedule: String,
            #[serde(with = "helium_lib::keypair::serde_pubkey")]
            cron_job: Pubkey,
            cron_jobs: u32,
            balance: token::TokenAmount,
        }

        let wallet = opts.maybe_wallet_key(None)?;
        let client = opts.client()?;

        let cron_job_key = schedule::cron_job_key_for_wallet(&wallet, 0);
        let Some(cronjob) = schedule::get(&client, &cron_job_key).await? else {
            bail!("No schedule found for this wallet");
        };

        let info = CronJobInfo {
            schedule: cronjob.schedule,
            cron_job: cron_job_key,
            cron_jobs: cronjob.num_transactions,
            balance: schedule::cron_job_balance(&client, &cron_job_key).await?,
        };

        print_json(&info)
    }
}

/// Remove the schedule for this wallet
///
/// This will close all active jobs in this schedule and then close the schedule itself.
#[derive(Clone, Debug, clap::Args)]
pub struct CloseCmd {
    /// Optionally fund the initialized schedule with the given amount of SOL
    #[command(flatten)]
    commit: CommitOpts,
}

impl CloseCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);

        let cron_job_key = schedule::cron_job_key_for_wallet(&keypair.pubkey(), 0);

        let (tx, _) = schedule::close(
            &client,
            &cron_job_key,
            0,
            SCHEDULE_NAME,
            &keypair,
            &transaction_opts,
        )
        .await?;

        print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
    }
}

/// Create and start a repating claim for all assets in a wallet using Tuktuk
///
/// The tuktuk system will fund the "claim_wallet" it uses to pay for claims
/// with a small amount of SOL. When new hotspots are added, additional payee
/// creation costs are incurred for that wallet.
///
/// Use the `--info` option in this command to check on the balance of the
/// claim_wallet. The suggested funded amount at the time of this writing is
/// between 0.05 and 0.1 SOL, with the top end allowing for a lot of growth.
#[derive(Clone, Debug, clap::Args)]
pub struct ClaimWalletCmd {
    /// The wallet to claim all hotspots for.
    /// Defaults to active wallet
    pub wallet: Option<Pubkey>,
    /// Get the claim wallet balance
    #[arg(long)]
    pub info: bool,
    #[command(flatten)]
    commit: CommitOpts,
}

impl ClaimWalletCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = opts.maybe_wallet_key(self.wallet)?;
        let client = opts.client()?;

        if self.info {
            let claim_wallet = queue::claim_wallet_key(&queue::TASK_QUEUE_ID, &wallet);
            let claim_info = json!({
                "claim_wallet": token::balance_for_address(&client, &claim_wallet).await?,
            });

            return print_json(&claim_info);
        }

        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let cron_job_key = schedule::cron_job_key_for_wallet(&keypair.pubkey(), 0);
        let (tx, _) =
            schedule::claim_wallet(&client, &cron_job_key, &wallet, &keypair, &transaction_opts)
                .await?;

        print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
    }
}
