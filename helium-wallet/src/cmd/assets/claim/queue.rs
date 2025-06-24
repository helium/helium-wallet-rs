use crate::cmd::*;
use helium_lib::{keypair::Pubkey, queue, token};

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

/// Queue claim transactions with Tuktuk
#[derive(Debug, Clone, clap::Subcommand)]
pub enum Command {
    Wallet(ClaimWalletCmd),
    Info(InfoCmd),
}

impl Command {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Wallet(cmd) => cmd.run(opts).await,
            Self::Info(cmd) => cmd.run(opts).await,
        }
    }
}

/// Create and start a one time claim for all assets in a wallet using Tuktuk
///
/// The tuktuk system will fund the "claim_wallet" it uses to pay for claims
/// with a small amount of SOL. When new hotspots are added, additional payee
/// creation costs are incurred for that wallet.
///
/// Use the `queue info` option in this command to check on the balance of the
/// claim_wallet. The suggested funded amount at the time of this writing is
/// between 0.05 and 0.1 SOL, with the top end allowing for a lot of growth.
#[derive(Clone, Debug, clap::Args)]
pub struct ClaimWalletCmd {
    /// The wallet to claim all hotspots for.
    /// Defaults to active wallet
    pub wallet: Option<Pubkey>,
    /// Commit the claim request transaction.
    #[command(flatten)]
    commit: CommitOpts,
}

impl ClaimWalletCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = opts.maybe_wallet_key(self.wallet)?;
        let client = opts.client()?;

        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let (tx, _) = queue::claim_wallet(
            &client,
            &queue::TASK_QUEUE_ID,
            &wallet,
            &keypair,
            &transaction_opts,
        )
        .await?;

        print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
    }
}

/// Displays information about the queue for this wallet
///
/// This includes the current balance of the claim wallet funding the claims
#[derive(Clone, Debug, clap::Args)]
pub struct InfoCmd {
    /// The wallet to look up claim information for
    /// Defaults to active wallet
    pub wallet: Option<Pubkey>,
}

impl InfoCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        #[derive(Debug, serde::Serialize, Default)]
        struct Info {
            claim_wallet: token::TokenBalance,
        }

        let wallet = opts.maybe_wallet_key(self.wallet)?;
        let client = opts.client()?;

        let claim_wallet = queue::claim_wallet_key(&queue::TASK_QUEUE_ID, &wallet);

        let info = Info {
            claim_wallet: token::balance_for_address(&client, &claim_wallet)
                .await?
                .unwrap_or(token::Token::Sol.to_balance(claim_wallet, 0)),
        };

        print_json(&info)
    }
}
