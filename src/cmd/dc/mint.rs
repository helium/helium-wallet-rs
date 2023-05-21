use crate::{
    cmd::{
        get_wallet_password, load_wallet, new_client, print_commit_result,
        print_simulation_response, Opts,
    },
    keypair::Pubkey,
    result::{anyhow, Result},
    token::{Token, TokenAmount},
};

#[derive(Debug, Clone, clap::Args)]
/// Burn HNT to Data Credits (DC) from this wallet to given payees wallet.
pub struct Cmd {
    /// Account address to send the resulting DC to. Defaults to the active
    /// wallet.
    #[arg(long)]
    payee: Option<Pubkey>,

    /// Amount of HNT to convert to dc
    #[arg(long, conflicts_with = "dc")]
    hnt: Option<f64>,

    /// Amount of DC to create from the HNT in the wallet
    #[arg(long, conflicts_with = "hnt")]
    dc: Option<u64>,

    /// Commit the burn
    #[arg(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let amount = match (self.hnt, self.dc) {
            (Some(hnt), None) => TokenAmount::from_f64(Token::Hnt, hnt),
            (None, Some(dc)) => TokenAmount::from_u64(Token::Dc, dc),
            _ => return Err(anyhow!("Must specify either HNT or DC")),
        };

        let tx = client.mint_dc(amount, &wallet.public_key, keypair)?;
        if self.commit {
            let signature = client.send_and_confirm_transaction(&tx)?;
            print_commit_result(signature)
        } else {
            let result = client.simulate_transaction(&tx)?;
            print_simulation_response(&result)
        }
    }
}
