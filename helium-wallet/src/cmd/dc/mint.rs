use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::DcMintRequest,
    keypair::{Pubkey, Signer},
    token::{Token, TokenAmount},
};

#[derive(Debug, Clone, clap::Args)]
/// Mint HNT to Data Credits (DC) from this wallet to a given payee's wallet.
///
/// Either the amount of HNT to burn or the amount of DC expected after the burn
/// can be specified.
pub struct Cmd {
    /// Account address to send the resulting DC to. Defaults to the active
    /// wallet.
    #[arg(long)]
    payee: Option<Pubkey>,

    /// Amount of HNT to convert to DC
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
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let amount = match (self.hnt, self.dc) {
            (Some(hnt), None) => TokenAmount::from_f64(Token::Hnt, hnt)?,
            (None, Some(dc)) => TokenAmount::from_u64(Token::Dc, dc),
            _ => return Err(anyhow!("Must specify either HNT or DC")),
        };

        // The API takes either the DC or the HNT amount; `amount` already
        // carries the value in the correct base unit for whichever was given.
        // (Minting has no Squads propose mode: it needs a fresh Pyth price in
        // the same transaction, which a deferred proposal can't provide.)
        let (dc_amount, hnt_amount) = match amount.token {
            Token::Dc => (Some(amount.amount.to_string()), None),
            Token::Hnt => (None, Some(amount.amount.to_string())),
            other => return Err(anyhow!("unexpected token {other:?} for dc mint")),
        };
        let api = opts.blockchain_api()?;
        let response = api
            .dc_mint(&DcMintRequest {
                owner: signer.pubkey().to_string(),
                dc_amount,
                hnt_amount,
                recipient: self.payee.map(|payee| payee.to_string()),
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(
                    &api,
                    &client,
                    &response,
                    &*signer,
                    ApiSigning::FreshBlockhash,
                )
                .await?
                .to_json(),
        )
    }
}
