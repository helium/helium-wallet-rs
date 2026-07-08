use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::ClaimRewardsRequest,
    keypair::{Pubkey, Signer},
    queue, token,
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

/// Queue a one-time claim with Tuktuk
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

/// Queue a one-time claim of all this wallet's hotspots using Tuktuk
///
/// Tuktuk funds the "claim_wallet" it uses to pay for claims with a small amount
/// of SOL. When new hotspots are added, additional payee creation costs are
/// incurred for that wallet.
///
/// Use `queue info` to check the balance of the claim_wallet. A funded amount
/// between 0.05 and 0.1 SOL leaves room for growth. The transaction is built by
/// the blockchain-api and signed locally.
#[derive(Clone, Debug, clap::Args)]
pub struct ClaimWalletCmd {
    /// Commit the claim request transaction.
    #[command(flatten)]
    commit: CommitOpts,
}

impl ClaimWalletCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .claim_rewards(&ClaimRewardsRequest {
                wallet_address: signer.pubkey().to_string(),
                // HNT with the Tuktuk path queues a one-time whole-wallet claim.
                network: None,
                tuktuk: Some(true),
                estimated_pending_rewards: None,
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

/// Display information about the claim queue for this wallet
///
/// This includes the current balance of the claim wallet funding the claims.
#[derive(Clone, Debug, clap::Args)]
pub struct InfoCmd {
    /// The wallet to look up claim information for.
    /// Defaults to the active wallet.
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
