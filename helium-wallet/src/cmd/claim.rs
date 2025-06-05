use crate::cmd::*;
use helium_lib::{crons, keypair::Pubkey, token};

#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: ClaimCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum ClaimCommand {
    Wallet(ClaimWalletCmd),
}

impl ClaimCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Wallet(cmd) => cmd.run(opts).await,
        }
    }
}

/// Create and start a one time claim for all hotspots in a wallet using Tuktuk
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
    /// Only get tuktuk claim wallet information
    #[arg(long)]
    pub info: bool,
    /// Commit the claim request transaction.
    #[command(flatten)]
    commit: CommitOpts,
}

impl ClaimWalletCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = if let Some(wallet) = self.wallet {
            wallet
        } else {
            opts.load_wallet()?.public_key
        };
        let client = opts.client()?;

        if self.info {
            let claim_wallet =
                crons::claim_wallet::claim_wallet_key(&crons::TASK_QUEUE_ID, &wallet);
            let claim_info = json!({
                "claim_wallet": token::balance_for_address(&client, &claim_wallet).await?,
            });

            return print_json(&claim_info);
        }

        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let (tx, _) = crons::claim_wallet::claim_wallet(
            &client,
            &crons::TASK_QUEUE_ID,
            &wallet,
            &keypair,
            &transaction_opts,
        )
        .await?;

        print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
    }
}
