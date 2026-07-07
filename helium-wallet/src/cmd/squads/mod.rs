use crate::cmd::*;
use helium_lib::{
    blockchain_api::{
        types::{SquadsConfigAction, SquadsProposeConfigChangeRequest},
        Client,
    },
    keypair::{Pubkey, Signer},
    message, solana_sdk,
    squads::{self as lib_squads, MemberAction, MultisigKey, VaultKey},
    transaction::mk_transaction,
    TransactionOpts,
};
use solana_sdk::{instruction::Instruction, transaction::VersionedTransaction};

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

/// Resolve a `--squads <addr>` value into the (multisig, vault PDA,
/// vault_index) triple every proposer-side wallet command needs.
/// Helium multisigs use vault index 0; non-zero vaults aren't surfaced
/// at the wallet CLI level.
pub(crate) async fn squads_vault<C: AsRef<helium_lib::client::SolanaRpcClient>>(
    client: &C,
    squads_target: Pubkey,
) -> Result<(MultisigKey, VaultKey, u8)> {
    let multisig = lib_squads::resolve_to_multisig(client, &squads_target).await?;
    let vault_index: u8 = 0;
    let vault = lib_squads::v4::vault_pda(&multisig, vault_index);
    Ok((multisig, vault, vault_index))
}

/// Resolve a `--squads <target>` (a multisig or vault PDA) to the multisig PDA
/// string the blockchain-api propose endpoints expect. Keeps the vault/target
/// resolution (a read) local while construction moves to the API.
pub(crate) async fn resolve_multisig<C: AsRef<helium_lib::client::SolanaRpcClient>>(
    client: &C,
    squads_target: Pubkey,
) -> Result<String> {
    Ok(lib_squads::resolve_to_multisig(client, &squads_target)
        .await?
        .to_string())
}

/// End-to-end Squads proposal submission for `--squads`-aware wallet
/// commands. Resolves the vault, runs `build_ixs(vault)` to produce
/// the inner instruction list, wraps the result as a v4 proposal, and
/// commits / prints the response. The closure is async so callers can
/// fetch on-chain state (assets, oracles) while building their ixs.
///
/// The printed JSON is augmented with `multisig`, `vault`,
/// `vault_index`, and `transaction_index` so reviewers can immediately
/// `helium-wallet squads inspect <vault> --index <n>` to verify what
/// they just signed lands as the on-chain proposal expects.
pub(crate) async fn submit_proposal_with<C, F, Fut>(
    client: &C,
    squads_target: Pubkey,
    memo: Option<String>,
    keypair: &dyn Signer,
    commit: &CommitOpts,
    txn_opts: &TransactionOpts,
    build_ixs: F,
) -> Result
where
    C: AsRef<helium_lib::client::SolanaRpcClient>,
    F: FnOnce(VaultKey) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<Instruction>>>,
{
    let (multisig, vault, vault_index) = squads_vault(client, squads_target).await?;
    // Pre-flight: the proposer (this wallet) must be a v4 member with
    // Initiate permission. v3 has no permission bits but still
    // requires membership. Cheaper to surface here than after the
    // user pays for the inner-ix build (asset fetches, etc.).
    let proposer = keypair.pubkey();
    lib_squads::check_member_permission(client, &multisig, &proposer, MemberAction::Initiate)
        .await?;
    let inner_ixs = build_ixs(vault).await?;
    let (tx, transaction_index) = wrap_as_proposal(
        client,
        multisig,
        vault_index,
        &inner_ixs,
        memo,
        keypair,
        txn_opts,
    )
    .await?;
    let response = commit.maybe_commit(tx, client).await?;
    let mut json = response.to_json();
    if let serde_json::Value::Object(map) = &mut json {
        map.insert("multisig".to_string(), multisig.to_string().into());
        map.insert("vault".to_string(), vault.to_string().into());
        map.insert("vault_index".to_string(), vault_index.into());
        map.insert("transaction_index".to_string(), transaction_index.into());
    }
    print_json(&json)
}

/// Sibling of `submit_proposal_with` for ConfigTransaction proposals
/// (member changes, threshold changes, time-lock, etc). Builds the
/// `[config_transaction_create, proposal_create]` pair, signs as
/// proposer, and surfaces the same `multisig` + `transaction_index`
/// JSON augment so reviewers can immediately `squads inspect <tx>`
/// post-submit. No vault index — config proposals act on the
/// multisig itself, not a vault.
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
        .commit_via_api(api, client, &response, keypair)
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

/// Wrap a list of inner instructions (built with `vault` as the
/// authority/payer) into a v4 Squads proposal: a `vault_transaction_create`
/// followed by `proposal_create`. Returns the outer transaction signed
/// by the proposer keypair (ready for `CommitOpts::maybe_commit`)
/// alongside the proposal's `transaction_index`, which the caller
/// needs to surface so reviewers can `squads inspect` post-submit.
///
/// `txn_opts.lut_addresses` is passed through on both sides — the inner
/// (proposal payload) compactor consults the LUTs to keep the encoded
/// `transaction_message` under the 1232-byte limit, and the outer
/// transaction message uses them for its own compression. The default
/// is Helium's `COMMON_LUT` (devnet variant on devnet).
pub(crate) async fn wrap_as_proposal<C: AsRef<helium_lib::client::SolanaRpcClient>>(
    client: &C,
    multisig: MultisigKey,
    vault_index: u8,
    inner_ixs: &[Instruction],
    memo: Option<String>,
    keypair: &dyn Signer,
    txn_opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64)> {
    let proposer = keypair.pubkey();
    let (proposal_ixs, transaction_index) = lib_squads::v4::propose_ixs_with_luts(
        client,
        multisig,
        vault_index,
        proposer,
        inner_ixs,
        memo,
        &txn_opts.lut_addresses,
    )
    .await?;
    let (msg, _block_height) =
        message::mk_message(client, &proposal_ixs, &txn_opts.lut_addresses, &proposer).await?;
    let tx = mk_transaction(msg, &[keypair])?;
    Ok((tx, transaction_index))
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
