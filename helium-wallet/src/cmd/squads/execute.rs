use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::SquadsExecuteProposalRequest,
    keypair::{Pubkey, Signer},
    message,
    squads::{self, MemberAction, SquadsError, Version},
    transaction::mk_transaction,
};

/// Execute an approved Squads proposal. The wallet must hold a member
/// keypair with `Execute` permission (v4) or be a member of the
/// multisig (v3).
#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    /// Multisig PDA, vault PDA, or transaction/proposal PDA.
    target: Pubkey,
    /// Transaction index. Required if `target` is a multisig or vault.
    #[arg(long)]
    index: Option<u64>,
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let txn_opts = self.commit.transaction_opts(&client);
        let member = signer.pubkey();

        let resolved = squads::resolve_proposal_target(&client, &self.target, self.index).await?;
        // Pre-flight: v4 requires the Execute permission; v3 has no
        // per-member permissions but still requires membership. Catches
        // "wrong wallet" before the on-chain program rejects.
        squads::check_member_permission(
            &client,
            &resolved.multisig,
            &member,
            MemberAction::Execute,
        )
        .await?;

        match resolved.version {
            // v4 executes build via the blockchain-api, which detects vault vs
            // config transactions server-side.
            Version::V4 => {
                let api = opts.blockchain_api()?;
                let response = api
                    .execute_proposal(&SquadsExecuteProposalRequest {
                        member: member.to_string(),
                        multisig: resolved.multisig.to_string(),
                        transaction_index: resolved.index.to_string(),
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
            // v3 (legacy) still builds locally: the API is v4-only.
            Version::V3 => {
                let index = resolved.index;
                let idx =
                    u32::try_from(index).map_err(|_| SquadsError::v3_index_out_of_range(index))?;
                let ix =
                    squads::v3::execute_transaction_ix(&client, resolved.multisig, idx, member)
                        .await?;
                let (msg, _block_height) =
                    message::mk_message(&client, &[ix], &txn_opts.lut_addresses, &member).await?;
                let tx = mk_transaction(msg, &[&*signer])?;
                print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
            }
        }
    }
}
