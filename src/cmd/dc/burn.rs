use crate::{
    cmd::{get_wallet_password, load_wallet, new_client, CommitOpts, Opts},
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
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;

        let keypair = wallet.decrypt(password.as_bytes())?;
        let tx = client.burn_dc(self.dc, keypair)?;
        self.commit.maybe_commit(&tx, &client)
    }
}
