use crate::cmd::{squads as cmd_squads, *};
use helium_lib::{keypair::Pubkey, squads::v4::ConfigActionInput};

/// Propose a new approval threshold for the multisig (v4 only). Once
/// the proposal lands and is executed, future proposals require the
/// new threshold to pass. Squads' on-chain handler enforces a
/// `[1, members.len()]` range at execute time.
#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    /// Multisig, vault, or any transaction/proposal PDA in the multisig.
    target: Pubkey,

    /// New approval threshold.
    new_threshold: u16,

    /// Memo recorded on the proposal.
    #[arg(long)]
    memo: Option<String>,

    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;
        let txn_opts = self.commit.transaction_opts(&client);

        let action = ConfigActionInput::ChangeThreshold {
            new_threshold: self.new_threshold,
        };
        cmd_squads::submit_config_proposal(
            &client,
            self.target,
            vec![action],
            self.memo.clone(),
            &keypair,
            &self.commit,
            &txn_opts,
        )
        .await
    }
}
