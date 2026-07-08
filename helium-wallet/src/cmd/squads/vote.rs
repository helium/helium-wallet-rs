use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::SquadsProposalVoteRequest,
    keypair::{Pubkey, Signer},
    squads::{self, MemberAction, Version},
};

/// Approve a Squads proposal. Without `--commit`, simulates only.
#[derive(Debug, Clone, clap::Args)]
pub struct Approve {
    #[command(flatten)]
    target: VoteTarget,
}

impl Approve {
    pub async fn run(&self, opts: Opts) -> Result {
        self.target.run(opts, VoteKind::Approve).await
    }
}

/// Reject a Squads proposal.
#[derive(Debug, Clone, clap::Args)]
pub struct Reject {
    #[command(flatten)]
    target: VoteTarget,
}

impl Reject {
    pub async fn run(&self, opts: Opts) -> Result {
        self.target.run(opts, VoteKind::Reject).await
    }
}

/// Cancel a previously-approved Squads proposal (only valid against an
/// `Approved` proposal that hasn't been executed yet).
#[derive(Debug, Clone, clap::Args)]
pub struct Cancel {
    #[command(flatten)]
    target: VoteTarget,
}

impl Cancel {
    pub async fn run(&self, opts: Opts) -> Result {
        self.target.run(opts, VoteKind::Cancel).await
    }
}

/// Common argument shape for all three vote actions.
#[derive(Debug, Clone, clap::Args)]
pub struct VoteTarget {
    /// Multisig PDA, vault PDA, or transaction/proposal PDA. Same shapes
    /// `squads inspect` accepts.
    target: Pubkey,
    /// Transaction index. Required if `target` is a multisig or vault;
    /// inferred from the body otherwise.
    #[arg(long)]
    index: Option<u64>,
    /// Optional memo string attached to the vote.
    #[arg(long)]
    memo: Option<String>,
    #[command(flatten)]
    commit: CommitOpts,
}

#[derive(Clone, Copy)]
enum VoteKind {
    Approve,
    Reject,
    Cancel,
}

impl VoteTarget {
    async fn run(&self, opts: Opts, kind: VoteKind) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;

        let resolved = squads::resolve_proposal_target(&client, &self.target, self.index).await?;
        let member = signer.pubkey();
        // Pre-flight: the on-chain program rejects votes from non-members
        // or members lacking the Vote permission. Surface a clear local
        // error instead of letting the user pay simulation fees on a
        // submission Squads will reject.
        squads::check_member_permission(&client, &resolved.multisig, &member, MemberAction::Vote)
            .await?;

        match resolved.version {
            // v4 votes build via the blockchain-api.
            Version::V4 => {
                let api = opts.blockchain_api()?;
                let req = SquadsProposalVoteRequest {
                    member: member.to_string(),
                    multisig: resolved.multisig.to_string(),
                    transaction_index: resolved.index.to_string(),
                    memo: self.memo.clone(),
                };
                let response = match kind {
                    VoteKind::Approve => api.approve_proposal(&req).await?,
                    VoteKind::Reject => api.reject_proposal(&req).await?,
                    VoteKind::Cancel => api.cancel_proposal(&req).await?,
                };
                print_json(
                    &self
                        .commit
                        .commit_via_api(&api, &client, &response, &*signer)
                        .await?
                        .to_json(),
                )
            }
            // v3 voting runs through v4-only paths; v3 multisigs are
            // inspect-only here.
            Version::V3 => bail!(
                "Squads v3 voting is not supported (v4 only). \
                 Inspect v3 multisigs with `squads inspect` and `squads list`."
            ),
        }
    }
}
