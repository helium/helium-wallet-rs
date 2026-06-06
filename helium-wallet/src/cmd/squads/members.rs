use crate::{
    cmd::{squads as cmd_squads, *},
    contacts,
};
use helium_lib::{
    keypair::Pubkey,
    squads::{self, v4::ConfigActionInput, MemberInfo, MemberPermissions, MultisigInfo},
};
use serde::Serialize;

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
    /// Also accepts a contact name.
    #[arg(value_parser = contacts::parse_address_or_name)]
    target: Pubkey,
}

impl ListCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let info = squads::get_multisig_info(&client, &self.target).await?;
        print_json(&NamedMultisigInfo::new(&info))
    }
}

/// Display wrapper for `MultisigInfo` that annotates each member with
/// their contact-book name when one is known. JSON shape matches the
/// upstream `MultisigInfo` except for the per-member `name` field,
/// which is omitted when no contact is found — existing consumers see
/// no change.
#[derive(Serialize)]
struct NamedMultisigInfo<'a> {
    address: &'a squads::MultisigKey,
    version: &'a squads::Version,
    threshold: u16,
    transaction_index: u64,
    members: Vec<NamedMember<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_from_vault: Option<&'a squads::VaultKey>,
}

#[derive(Serialize)]
struct NamedMember<'a> {
    #[serde(flatten)]
    inner: &'a MemberInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
}

impl<'a> NamedMultisigInfo<'a> {
    fn new(info: &'a MultisigInfo) -> Self {
        let book = contacts::cached();
        Self {
            address: &info.address,
            version: &info.version,
            threshold: info.threshold,
            transaction_index: info.transaction_index,
            members: info
                .members
                .iter()
                .map(|m| NamedMember {
                    inner: m,
                    name: book.find_by_address(&m.key).map(|c| c.name.as_str()),
                })
                .collect(),
            resolved_from_vault: info.resolved_from_vault.as_ref(),
        }
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
    /// Also accepts a contact name.
    #[arg(value_parser = contacts::parse_address_or_name)]
    target: Pubkey,

    /// Pubkey of the new member. Also accepts a contact name.
    #[arg(value_parser = contacts::parse_address_or_name)]
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
    /// Also accepts a contact name.
    #[arg(value_parser = contacts::parse_address_or_name)]
    target: Pubkey,

    /// Pubkey of the member to remove. Also accepts a contact name.
    #[arg(value_parser = contacts::parse_address_or_name)]
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
