use crate::cmd::*;
use helium_lib::{
    keypair::{Pubkey, Signer},
    message, solana_sdk,
    squads::{self, MemberAction, ProposalTarget, SquadsError, Version},
    transaction::mk_transaction,
};

/// Approve a Squads proposal. Without `--commit`, simulates only.
#[derive(Debug, Clone, clap::Args)]
pub struct Approve {
    #[command(flatten)]
    target: VoteTarget,
    /// Bundle the matching `*_transaction_execute` instruction into the
    /// same transaction as the approve, so the proposal is approved AND
    /// executed atomically with a single signature. Convenient for
    /// single-threshold multisigs and "I'm the last vote" scenarios.
    /// Errors out if the multisig has a non-zero `time_lock` (the
    /// execute handler enforces `now - approval_ts >= time_lock`, which
    /// can't hold inside the same block).
    #[arg(long)]
    execute: bool,
}

impl Approve {
    pub async fn run(&self, opts: Opts) -> Result {
        if !self.execute {
            return self.target.run(opts, VoteKind::Approve).await;
        }
        self.run_with_execute(opts).await
    }

    async fn run_with_execute(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;
        let txn_opts = self.target.commit.transaction_opts(&client);
        let member = keypair.pubkey();

        let resolved =
            squads::resolve_proposal_target(&client, &self.target.target, self.target.index)
                .await?;
        if !matches!(resolved.version, Version::V4) {
            bail!("--execute on `squads approve` is only supported for v4 multisigs");
        }

        // Caller needs both Vote and Execute permissions for the combined tx
        // to land. Check up front so the user gets a clean local error
        // instead of paying simulation fees on a doomed submission.
        squads::check_member_permission(&client, &resolved.multisig, &member, MemberAction::Vote)
            .await?;
        squads::check_member_permission(
            &client,
            &resolved.multisig,
            &member,
            MemberAction::Execute,
        )
        .await?;

        let time_lock = squads::v4::get_time_lock(&client, &resolved.multisig).await?;
        if time_lock != 0 {
            bail!(
                "multisig has a {time_lock}s time lock; --execute can't ride along with \
                 approve in the same transaction. Run `squads execute` separately after \
                 the time lock releases."
            );
        }

        let approve_ix = build_vote_ix(&resolved, member, &self.target.memo, VoteKind::Approve)?;
        let execute_ix =
            squads::v4::execute_ix(&client, resolved.multisig, resolved.index, member).await?;

        let ixs = &[approve_ix, execute_ix];
        let (msg, _block_height) =
            message::mk_message(&client, ixs, &txn_opts.lut_addresses, &member).await?;
        let tx = mk_transaction(msg, &[&*keypair])?;
        print_json(
            &self
                .target
                .commit
                .maybe_commit(tx, &client)
                .await?
                .to_json(),
        )
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
    /// Optional v4 memo string attached to the vote (ignored on v3).
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
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;
        let txn_opts = self.commit.transaction_opts(&client);

        let resolved = squads::resolve_proposal_target(&client, &self.target, self.index).await?;
        let member = keypair.pubkey();
        // Pre-flight: the on-chain program rejects votes from non-members
        // or members lacking the Vote permission. Surface a clear local
        // error instead of letting the user pay simulation fees on a
        // submission Squads will reject.
        squads::check_member_permission(&client, &resolved.multisig, &member, MemberAction::Vote)
            .await?;
        let vote_ix = build_vote_ix(&resolved, member, &self.memo, kind)?;

        let ixs = &[vote_ix];
        let (msg, _block_height) =
            message::mk_message(&client, ixs, &txn_opts.lut_addresses, &member).await?;
        let tx = mk_transaction(msg, &[&*keypair])?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}

fn build_vote_ix(
    resolved: &ProposalTarget,
    member: Pubkey,
    memo: &Option<String>,
    kind: VoteKind,
) -> Result<solana_sdk::instruction::Instruction> {
    let multisig = resolved.multisig;
    Ok(match resolved.version {
        Version::V4 => {
            let idx = resolved.index;
            let memo = memo.clone();
            match kind {
                VoteKind::Approve => squads::v4::proposal_approve_ix(multisig, idx, member, memo)?,
                VoteKind::Reject => squads::v4::proposal_reject_ix(multisig, idx, member, memo)?,
                VoteKind::Cancel => squads::v4::proposal_cancel_ix(multisig, idx, member, memo)?,
            }
        }
        Version::V3 => {
            let idx = v3_index(resolved.index)?;
            match kind {
                VoteKind::Approve => squads::v3::approve_transaction_ix(multisig, idx, member),
                VoteKind::Reject => squads::v3::reject_transaction_ix(multisig, idx, member),
                VoteKind::Cancel => squads::v3::cancel_transaction_ix(multisig, idx, member),
            }
        }
    })
}

fn v3_index(index: u64) -> Result<u32> {
    u32::try_from(index).map_err(|_| SquadsError::v3_index_out_of_range(index).into())
}
