use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::DcMintRequest,
    keypair::{Pubkey, Signer},
    programs::KnownProgram,
    token::{Token, TokenAmount},
    transaction::VersionedTransaction,
    verify,
};

/// `mint_data_credits_v0` names the wallet the DC is credited to at account
/// index 4, and the wallet whose HNT is burned to pay for it at index 5.
const MINT_METHODS: &[&str] = &["mint_data_credits_v0"];
const RECIPIENT_ACCOUNT_INDEX: usize = 4;
const OWNER_ACCOUNT_INDEX: usize = 5;

/// Refuse to sign unless the DC is credited to the payee that was asked for,
/// paid for by this wallet.
///
/// DC is non-transferable once minted, so a substituted recipient is not
/// recoverable: the HNT is burned from this wallet either way and the credits
/// land somewhere else permanently.
fn assert_mints_to(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    recipient: &Pubkey,
) -> Result<()> {
    let found = verify::find_methods(unsigned, KnownProgram::DataCredits, MINT_METHODS)?;
    let [mint] = found.as_slice() else {
        bail!(
            "expected exactly one data-credits mint, found {}; refusing to sign",
            found.len()
        );
    };
    let credited = mint.account(RECIPIENT_ACCOUNT_INDEX)?;
    if credited != *recipient {
        bail!("the mint would credit {credited}, not the requested {recipient}");
    }
    let owner = mint.account(OWNER_ACCOUNT_INDEX)?;
    if owner != *wallet {
        bail!("the mint burns HNT from {owner}, not this wallet");
    }
    Ok(())
}

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
        let wallet = signer.pubkey();
        assert_mints_to(
            &response.decode_transactions()?,
            &wallet,
            &self.payee.unwrap_or(wallet),
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use helium_lib::solana_sdk::{
        instruction::{AccountMeta, Instruction},
        message::{Message, VersionedMessage},
    };

    /// `sha256("global:mint_data_credits_v0")[..8]`.
    const MINT_DISCRIMINATOR: [u8; 8] = [78, 109, 169, 132, 144, 94, 221, 57];

    /// A `mint_data_credits_v0` with the account layout the IDL declares, so
    /// the indices the guard reads mean here what they mean on chain.
    fn mint_tx(recipient: Pubkey, owner: Pubkey) -> VersionedTransaction {
        let accounts = [
            Pubkey::new_unique(), // data_credits
            Pubkey::new_unique(), // hnt_price_oracle
            Pubkey::new_unique(), // burner
            Pubkey::new_unique(), // recipient_token_account
            recipient,
            owner,
            Pubkey::new_unique(), // hnt_mint
            Pubkey::new_unique(), // dc_mint
        ];
        let ix = Instruction {
            program_id: KnownProgram::DataCredits.id(),
            accounts: accounts
                .iter()
                .map(|key| AccountMeta::new(*key, false))
                .collect(),
            data: MINT_DISCRIMINATOR.to_vec(),
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&owner))),
        }
    }

    #[test]
    fn a_mint_to_the_requested_payee_is_accepted() {
        let wallet = Pubkey::new_unique();
        let payee = Pubkey::new_unique();
        assert_mints_to(&[mint_tx(payee, wallet)], &wallet, &payee).expect("the requested mint");
    }

    #[test]
    fn a_mint_credited_elsewhere_is_refused() {
        let wallet = Pubkey::new_unique();
        let payee = Pubkey::new_unique();
        let attacker = Pubkey::new_unique();
        let err = assert_mints_to(&[mint_tx(attacker, wallet)], &wallet, &payee)
            .expect_err("a substituted payee must be refused");
        assert!(err.to_string().contains("not the requested"), "{err}");
    }

    #[test]
    fn a_mint_burning_another_wallets_hnt_is_refused() {
        let wallet = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        let payee = Pubkey::new_unique();
        let err = assert_mints_to(&[mint_tx(payee, other)], &wallet, &payee)
            .expect_err("a mint paid for by another wallet must be refused");
        assert!(err.to_string().contains("not this wallet"), "{err}");
    }

    #[test]
    fn a_response_carrying_no_mint_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_mints_to(&[], &wallet, &wallet)
            .expect_err("an action that was never built must be refused");
        assert!(err.to_string().contains("found 0"), "{err}");
    }

    #[test]
    fn a_second_mint_smuggled_alongside_the_first_is_refused() {
        let wallet = Pubkey::new_unique();
        let payee = Pubkey::new_unique();
        let err = assert_mints_to(
            &[
                mint_tx(payee, wallet),
                mint_tx(Pubkey::new_unique(), wallet),
            ],
            &wallet,
            &payee,
        )
        .expect_err("a batch minting twice must be refused");
        assert!(err.to_string().contains("found 2"), "{err}");
    }
}
