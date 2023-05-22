use crate::{
    cmd::{
        get_wallet_password, load_wallet, new_client, print_commit_result,
        print_simulation_response, Opts,
    },
    dao::SubDao,
    result::Result,
};

#[derive(Debug, Clone, clap::Args)]
/// Delegate DC from this wallet to a given router
pub struct Cmd {
    /// Subdao to delegate DC to
    subdao: SubDao,

    /// Router helium public key to delegate to
    router: String,

    /// Amount of DC to delgate
    dc: u64,

    /// Commit the delegation
    #[arg(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let tx = client.delegate_dc(self.subdao, &self.router, self.dc, keypair)?;
        if self.commit {
            let signature = client.send_and_confirm_transaction(&tx)?;
            print_commit_result(signature)
        } else {
            let result = client.simulate_transaction(&tx)?;
            print_simulation_response(&result)
        }
    }
}
