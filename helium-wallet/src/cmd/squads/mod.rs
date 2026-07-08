use crate::cmd::*;
use helium_lib::{
    blockchain_api::{
        types::{SquadsConfigAction, SquadsProposeConfigChangeRequest},
        Client,
    },
    keypair::{Pubkey, Signer},
    squads::{self as lib_squads, MemberAction},
};

mod execute;
mod inspect;
mod list;
mod members;
mod threshold;
mod vote;

/// Shared `--squads`/`--memo` options for commands that can submit their
/// transaction as a Squads v4 proposal instead of executing directly.
/// Flatten with `#[command(flatten)]`.
#[derive(Debug, Clone, clap::Args)]
pub struct SquadsOpts {
    /// Submit as a Squads v4 proposal instead of executing directly.
    /// Accepts a multisig PDA or a vault PDA — when a vault is given the
    /// multisig is resolved through the local cache. The transaction's
    /// authority becomes the resolved vault (not the wallet), and the
    /// wallet just signs as proposer.
    #[arg(long)]
    pub squads: Option<Pubkey>,
    /// Memo recorded on the v4 proposal (`--squads` only).
    #[arg(long)]
    pub memo: Option<String>,
}

impl SquadsOpts {
    /// Resolve the propose-mode fields for an action command. When `--squads`
    /// is set, resolve the target to its multisig and verify `proposer` holds
    /// Initiate permission before it's used — so a non-member fails here rather
    /// than after building, signing, and submitting a doomed proposal. Returns
    /// `(multisig, memo)` for the action request, or `(None, None)` when
    /// `--squads` is absent.
    pub(crate) async fn resolve<C: AsRef<helium_lib::client::SolanaRpcClient>>(
        &self,
        client: &C,
        proposer: &Pubkey,
    ) -> Result<(Option<String>, Option<String>)> {
        let Some(target) = self.squads else {
            return Ok((None, None));
        };
        let multisig = lib_squads::resolve_to_multisig(client, &target).await?;
        lib_squads::check_member_permission(client, &multisig, proposer, MemberAction::Initiate)
            .await?;
        Ok((Some(multisig.to_string()), self.memo.clone()))
    }
}

/// Submit a ConfigTransaction proposal (member changes, threshold changes) to
/// the blockchain-api and commit it. Surfaces the `multisig` and the
/// server-assigned `transaction_index` in the JSON output so reviewers can
/// `squads inspect <multisig> --index <n>` the proposal. Config proposals act
/// on the multisig itself, not a vault.
pub(crate) async fn submit_config_proposal<C>(
    client: &C,
    api: &Client,
    target: Pubkey,
    actions: Vec<SquadsConfigAction>,
    memo: Option<String>,
    keypair: &(dyn Signer + Send + Sync),
    commit: &CommitOpts,
) -> Result
where
    C: AsRef<helium_lib::client::SolanaRpcClient>,
{
    let multisig = lib_squads::resolve_to_multisig(client, &target).await?;
    let proposer = keypair.pubkey();
    // Same Initiate-permission gate the vault-tx proposer side uses. Config
    // proposals are v4-only, matching the API.
    lib_squads::check_member_permission(client, &multisig, &proposer, MemberAction::Initiate)
        .await?;
    let response = api
        .propose_config_change(&SquadsProposeConfigChangeRequest {
            member: proposer.to_string(),
            multisig: multisig.to_string(),
            actions,
            memo,
        })
        .await?;
    // The server assigns the proposal's transaction index and returns it in the
    // response's actionMetadata; surface it (plus the multisig) so reviewers can
    // immediately `squads inspect` the proposal post-submit.
    let transaction_index = response
        .action_metadata
        .as_ref()
        .and_then(|m| m.get("transactionIndex"))
        .cloned();
    let commit_response = commit
        .commit_via_api(api, client, &response, keypair, ApiSigning::FreshBlockhash)
        .await?;
    let mut json = commit_response.to_json();
    if let serde_json::Value::Object(map) = &mut json {
        map.insert("multisig".to_string(), multisig.to_string().into());
        if let Some(idx) = transaction_index {
            map.insert("transaction_index".to_string(), idx);
        }
    }
    print_json(&json)
}

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: SquadsCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

/// Commands for Squads multisig wallets (v3 and v4)
#[derive(Debug, Clone, clap::Subcommand)]
pub enum SquadsCommand {
    Members(members::Cmd),
    List(list::Cmd),
    Inspect(inspect::Cmd),
    Approve(vote::Approve),
    Reject(vote::Reject),
    Cancel(vote::Cancel),
    Execute(execute::Cmd),
    Threshold(threshold::Cmd),
}

impl SquadsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Members(cmd) => cmd.run(opts).await,
            Self::List(cmd) => cmd.run(opts).await,
            Self::Inspect(cmd) => cmd.run(opts).await,
            Self::Approve(cmd) => cmd.run(opts).await,
            Self::Reject(cmd) => cmd.run(opts).await,
            Self::Cancel(cmd) => cmd.run(opts).await,
            Self::Execute(cmd) => cmd.run(opts).await,
            Self::Threshold(cmd) => cmd.run(opts).await,
        }
    }
}
