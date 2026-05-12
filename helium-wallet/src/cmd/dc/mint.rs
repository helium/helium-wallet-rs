use crate::cmd::{squads as cmd_squads, *};
use helium_lib::{
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

    /// Submit as a Squads v4 proposal — see `transfer one --squads`.
    /// HNT is sourced from the resolved vault; the wallet only signs as
    /// proposer.
    #[arg(long)]
    squads: Option<Pubkey>,
    /// Memo recorded on the v4 proposal (`--squads` only).
    #[arg(long)]
    memo: Option<String>,

    /// Commit the burn
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;

        let client = opts.client()?;
        let amount = match (self.hnt, self.dc) {
            (Some(hnt), None) => TokenAmount::from_f64(Token::Hnt, hnt),
            (None, Some(dc)) => TokenAmount::from_u64(Token::Dc, dc),
            _ => return Err(anyhow!("Must specify either HNT or DC")),
        };
        let transaction_opts = self.commit.transaction_opts(&client);

        if let Some(squads_target) = self.squads {
            let client_ref = &client;
            let payee_override = self.payee;
            return cmd_squads::submit_proposal_with(
                client_ref,
                squads_target,
                self.memo.clone(),
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

        let payee = self.payee.unwrap_or(signer.pubkey());
        let (tx, _) = dc::mint(&client, amount, &payee, &*signer, &transaction_opts).await?;
        print_json(&self.commit.maybe_commit(tx, &client).await?.to_json())
    }
}
