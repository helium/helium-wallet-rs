use crate::{
    cmd::{get_wallet_password, CommitOpts, Opts},
    dc,
    result::Result,
};

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
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let settings = opts.try_into()?;

        let tx = dc::burn(&settings, self.dc, keypair)?;
        self.commit.maybe_commit(&tx, &settings)
    }
}
