use crate::cmd::*;
use helium_lib::{keypair::Pubkey, squads};

/// Decode a Squads proposal (v3 or v4): status, votes, and the inner
/// instructions it will execute when approved. Use this before signing to
/// verify the proposal does what it claims.
///
/// Accepts either:
///   - a multisig or vault PDA plus an explicit `--index`, or
///   - a transaction or proposal PDA on its own (it self-identifies).
#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    /// Multisig PDA, vault PDA, transaction PDA, or proposal PDA.
    target: Pubkey,

    /// Transaction index. Required when `target` is a multisig or vault;
    /// inferred from the account body when `target` is a transaction or
    /// proposal PDA.
    #[arg(long)]
    index: Option<u64>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let info = squads::inspect_target(&client, &self.target, self.index).await?;
        print_json(&info)
    }
}
