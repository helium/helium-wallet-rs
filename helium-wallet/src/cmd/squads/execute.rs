use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::SquadsExecuteProposalRequest,
    keypair::{Pubkey, Signer},
    squads::{self, MemberAction, Version},
};

/// Execute an approved Squads v4 proposal. The wallet must hold a member
/// keypair with `Execute` permission.
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
        let member = signer.pubkey();

        let resolved = squads::resolve_proposal_target(&client, &self.target, self.index).await?;
        // Pre-flight: v4 requires the Execute permission. Catches "wrong wallet"
        // before the on-chain program rejects.
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
            // v3 execution runs through v4-only paths; v3 multisigs are
            // inspect-only here.
            Version::V3 => bail!(
                "Squads v3 execution is not supported (v4 only). \
                 Inspect v3 multisigs with `squads inspect` and `squads list`."
            ),
        }
    }
}
