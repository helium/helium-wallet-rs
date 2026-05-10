use crate::cmd::*;
use helium_lib::{
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
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;
        let txn_opts = self.commit.transaction_opts(&client);
        let member = keypair.pubkey();

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
        let ix = match resolved.version {
            Version::V4 => {
                // Dispatcher: routes to vault_transaction_execute or
                // config_transaction_execute based on the on-chain
                // transaction account's discriminator.
                squads::v4::execute_ix(&client, resolved.multisig, resolved.index, member).await?
            }
            Version::V3 => {
                let index = resolved.index;
                let idx =
                    u32::try_from(index).map_err(|_| SquadsError::v3_index_out_of_range(index))?;
                squads::v3::execute_transaction_ix(&client, resolved.multisig, idx, member).await?
            }
        };

        let ixs = &[ix];
        let (msg, _block_height) =
            message::mk_message(&client, ixs, &txn_opts.lut_addresses, &member).await?;
        let tx = mk_transaction(msg, &[&*keypair])?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
