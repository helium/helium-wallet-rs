use crate::{
    cmd::{get_wallet_password, load_wallet, new_client, CommitOpts, Opts},
    dao::SubDao,
    result::Result,
};

#[derive(Debug, Clone, clap::Args)]
/// Delegate DC from this wallet to a given router
pub struct Cmd {
    /// Subdao to delegate DC to
    subdao: SubDao,

    /// Public Helium payer key to delegate to
    payer: String,

    /// Amount of DC to delgate
    dc: u64,

    /// Commit the delegation
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let tx = client.delegate_dc(self.subdao, &self.payer, self.dc, keypair)?;
        self.commit.maybe_commit(&tx, &client)
    }
}
