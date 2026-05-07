use crate::cmd::{squads as cmd_squads, *};
use helium_lib::{
    keypair::Pubkey,
    squads::{self, v4::ConfigActionInput, MemberPermissions},
};

/// Manage the member roster of a Squads multisig.
#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: SubCmd,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match &self.cmd {
            SubCmd::List(c) => c.run(opts).await,
            SubCmd::Add(c) => c.run(opts).await,
            SubCmd::Remove(c) => c.run(opts).await,
        }
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum SubCmd {
    List(ListCmd),
    Add(AddCmd),
    Remove(RemoveCmd),
}

/// Print the members, threshold, and version of a Squads multisig.
/// `target` accepts a multisig PDA, a vault PDA, or any
/// transaction/proposal PDA in the multisig — vault and
/// transaction-bearing addresses resolve back to the multisig
/// automatically (vaults via cache + recent-signature scan,
/// transaction PDAs by reading the multisig from their body).
#[derive(Debug, Clone, clap::Args)]
pub struct ListCmd {
    /// Multisig, vault, or any transaction/proposal PDA in the multisig.
    target: Pubkey,
}

impl ListCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let info = squads::get_multisig_info(&client, &self.target).await?;
        print_json(&info)
    }
}

/// Propose adding a member to a Squads multisig (v4 only). Creates a
/// ConfigTransaction proposal — once approved + executed by threshold
/// of existing members, the new member is added to `multisig.members`
/// with the requested permissions, and `multisig.stale_transaction_index`
/// advances to invalidate any pending vault transactions created
/// against the old member set.
#[derive(Debug, Clone, clap::Args)]
pub struct AddCmd {
    /// Multisig, vault, or any transaction/proposal PDA in the multisig.
    target: Pubkey,

    /// Pubkey of the new member.
    new_member: Pubkey,

    /// Permissions the new member receives. Defaults to all three
    /// (initiate + vote + execute), the typical "trusted member"
    /// shape Helium multisigs use. Use repeated flags or a
    /// comma-separated list to narrow: e.g. `--perm vote --perm
    /// execute` for a member who can vote and execute but not
    /// initiate proposals.
    #[arg(long = "perm", value_enum, num_args = 0.., value_delimiter = ',')]
    perm: Vec<PermissionFlag>,

    /// Memo recorded on the proposal.
    #[arg(long)]
    memo: Option<String>,

    #[command(flatten)]
    commit: CommitOpts,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum PermissionFlag {
    Initiate,
    Vote,
    Execute,
}

impl PermissionFlag {
    /// Set the matching field on `perms` to true.
    fn apply(self, perms: &mut MemberPermissions) {
        match self {
            Self::Initiate => perms.propose = true,
            Self::Vote => perms.vote = true,
            Self::Execute => perms.execute = true,
        }
    }
}

impl AddCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let txn_opts = self.commit.transaction_opts(&client);

        let permissions = if self.perm.is_empty() {
            MemberPermissions::ALL
        } else {
            let mut perms = MemberPermissions::default();
            for flag in &self.perm {
                flag.apply(&mut perms);
            }
            perms
        };
        let action = ConfigActionInput::AddMember {
            new_member: self.new_member,
            permissions,
        };
        cmd_squads::submit_config_proposal(
            &client,
            self.target,
            vec![action],
            self.memo.clone(),
            &*signer,
            &self.commit,
            &txn_opts,
        )
        .await
    }
}

/// Propose removing a member from a Squads multisig (v4 only). Once
/// the proposal lands and is executed, the member is dropped from
/// `multisig.members` and `stale_transaction_index` advances.
/// Removing a member can require dropping the threshold first if the
/// remaining member count would fall below threshold — Squads'
/// on-chain handler will reject the execute in that case.
#[derive(Debug, Clone, clap::Args)]
pub struct RemoveCmd {
    /// Multisig, vault, or any transaction/proposal PDA in the multisig.
    target: Pubkey,

    /// Pubkey of the member to remove.
    old_member: Pubkey,

    /// Memo recorded on the proposal.
    #[arg(long)]
    memo: Option<String>,

    #[command(flatten)]
    commit: CommitOpts,
}

impl RemoveCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let txn_opts = self.commit.transaction_opts(&client);

        let action = ConfigActionInput::RemoveMember {
            old_member: self.old_member,
        };
        cmd_squads::submit_config_proposal(
            &client,
            self.target,
            vec![action],
            self.memo.clone(),
            &*signer,
            &self.commit,
            &txn_opts,
        )
        .await
    }
}
