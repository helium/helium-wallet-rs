use crate::cmd::*;
use helium_lib::{
    dc,
    keypair::Pubkey,
    token::{Token, TokenAmount},
};

#[derive(Debug, Clone, clap::Args)]
/// Mint HNT to Data Credits (DC) from this wallet to given payees wallet.
///
/// One of the amount of HNT to burn or the amount of DC expected after the burn
/// can be specified.
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
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = opts.load_wallet()?;
        let settings = opts.try_into()?;

        let payee = self.payee.as_ref().unwrap_or(&wallet.public_key);
        let amount = match (self.hnt, self.dc) {
            (Some(hnt), None) => TokenAmount::from_f64(Token::Hnt, hnt),
            (None, Some(dc)) => TokenAmount::from_u64(Token::Dc, dc),
            _ => return Err(anyhow!("Must specify either HNT or DC")),
        };

        let keypair = wallet.decrypt(password.as_bytes())?;
        let tx = dc::mint(&settings, amount, payee, keypair).await?;
        print_json(&self.commit.maybe_commit(&tx, &settings).await?.to_json())
    }
}
