use crate::cmd::{
    squads::{self as cmd_squads, SquadsOpts},
    *,
};
use helium_lib::{
    blockchain_api::types::DcMintRequest,
    dc,
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

    /// HNT is sourced from the resolved vault; the wallet only signs as
    /// proposer.
    #[command(flatten)]
    squads: SquadsOpts,

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

        if let Some(squads_target) = self.squads.squads {
            let transaction_opts = self.commit.transaction_opts(&client);
            let client_ref = &client;
            let payee_override = self.payee;
            return cmd_squads::submit_proposal_with(
                client_ref,
                squads_target,
                self.squads.memo.clone(),
                &*signer,
                &self.commit,
                &transaction_opts,
                |vault| async move {
                    // Default payee is the vault when --squads is set;
                    // the resulting DC lands in the vault's DC ATA
                    // unless --payee overrides.
                    let payee = payee_override.unwrap_or_else(|| vault.into_pubkey());
                    Ok(vec![
                        dc::mint_instruction(client_ref, amount, &payee, vault.as_pubkey()).await?,
                    ])
                },
            )
            .await;
        }

        // The API takes either the DC or the HNT amount; `amount` already
        // carries the value in the correct base unit for whichever was given.
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
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}
