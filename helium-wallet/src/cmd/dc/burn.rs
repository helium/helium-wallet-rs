use crate::cmd::*;
use helium_lib::dc;

#[derive(Debug, Clone, clap::Args)]
/// Burn Data Credits (DC) from this wallet into oblivion.
pub struct Cmd {
    /// Amount of DC to burn
    dc: u64,

    /// Commit the burn
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let settings = opts.try_into()?;

        let tx = dc::burn(&settings, self.dc, keypair).await?;
        print_json(&self.commit.maybe_commit(&tx, &settings).await?.to_json())
    }
}
