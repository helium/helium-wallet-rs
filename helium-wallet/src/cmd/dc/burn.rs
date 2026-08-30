use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::{
    blockchain_api::types::DcBurnRequest, keypair::Signer, programs::KnownProgram,
    transaction::VersionedTransaction, verify,
};

/// `burn_without_tracking_v0` carries the amount in its arguments and names the
/// wallet the DC is burned from at account index 2. Its accounts are declared as
/// one composite, which Anchor flattens into the instruction in that order.
const BURN_METHODS: &[&str] = &["burn_without_tracking_v0"];
const OWNER_ACCOUNT_INDEX: usize = 2;

/// Refuse to sign unless the burn destroys the amount that was asked for, from
/// this wallet.
fn assert_burns(unsigned: &[VersionedTransaction], wallet: &Pubkey, amount: u64) -> Result<()> {
    let found = verify::find_methods(unsigned, KnownProgram::DataCredits, BURN_METHODS)?;
    let [burn] = found.as_slice() else {
        bail!(
            "expected exactly one data-credits burn, found {}; refusing to sign",
            found.len()
        );
    };
    let owner = burn.account(OWNER_ACCOUNT_INDEX)?;
    if owner != *wallet {
        bail!("the burn destroys DC held by {owner}, not this wallet");
    }
    let args = burn
        .args()
        .ok_or_else(|| anyhow!("the burn's arguments could not be read; refusing to sign"))?;
    let got = args["args"]["amount"]
        .as_u64()
        .ok_or_else(|| anyhow!("the burn names no amount; refusing to sign"))?;
    if got != amount {
        bail!("the burn destroys {got} DC, not the requested {amount}");
    }
    Ok(())
}

/// A proposal burns the vault's DC inside a Squads instruction this cannot
/// read, so it is held to burning nothing of its own.
fn assert_wraps_no_burn(unsigned: &[VersionedTransaction]) -> Result<()> {
    if !verify::find_methods(unsigned, KnownProgram::DataCredits, BURN_METHODS)?.is_empty() {
        bail!("a Squads proposal must not burn DC at the top level; refusing to sign");
    }
    Ok(())
}

#[derive(Debug, Clone, clap::Args)]
/// Burn Data Credits (DC) from this wallet into oblivion.
pub struct Cmd {
    /// Amount of DC to burn
    dc: u64,
    /// Burn from the resolved vault's DC ATA as a Squads v4 proposal instead of
    /// from this wallet.
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

        // With `--squads` the API burns from the resolved vault as a proposal;
        // otherwise it burns directly from this wallet.
        let (multisig, memo) = self.squads.resolve(&client, &signer.pubkey()).await?;
        let is_proposal = multisig.is_some();

        let api = opts.blockchain_api()?;
        let response = api
            .dc_burn(&DcBurnRequest {
                owner: signer.pubkey().to_string(),
                amount: self.dc.to_string(),
                multisig,
                memo,
            })
            .await?;
        let unsigned = response.decode_transactions()?;
        if is_proposal {
            assert_wraps_no_burn(&unsigned)?;
        } else {
            assert_burns(&unsigned, &signer.pubkey(), self.dc)?;
        }
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
    use helium_lib::{
        keypair::Pubkey,
        solana_sdk::{
            instruction::{AccountMeta, Instruction},
            message::{Message, VersionedMessage},
        },
    };

    /// The discriminator the data-credits IDL declares for this method.
    const BURN_DISCRIMINATOR: [u8; 8] = [129, 106, 43, 4, 52, 143, 102, 208];

    /// The composite `burn_accounts` flattened as Anchor lays it out, so index
    /// 2 is the owner here as it is on chain.
    fn burn_tx(owner: Pubkey, amount: u64) -> VersionedTransaction {
        let mut data = BURN_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&amount.to_le_bytes());
        let accounts = [
            Pubkey::new_unique(), // data_credits
            Pubkey::new_unique(), // burner
            owner,
            Pubkey::new_unique(), // dc_mint
        ];
        let ix = Instruction {
            program_id: KnownProgram::DataCredits.id(),
            accounts: accounts
                .iter()
                .map(|key| AccountMeta::new(*key, false))
                .collect(),
            data,
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&owner))),
        }
    }

    #[test]
    fn the_requested_burn_is_accepted() {
        let wallet = Pubkey::new_unique();
        assert_burns(&[burn_tx(wallet, 1_000)], &wallet, 1_000).expect("the requested burn");
    }

    #[test]
    fn a_burn_of_another_amount_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_burns(&[burn_tx(wallet, 9_999)], &wallet, 1_000)
            .expect_err("a substituted amount must be refused");
        assert!(err.to_string().contains("9999"), "{err}");
    }

    #[test]
    fn a_burn_of_another_wallets_dc_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_burns(&[burn_tx(Pubkey::new_unique(), 1_000)], &wallet, 1_000)
            .expect_err("a burn from another wallet must be refused");
        assert!(err.to_string().contains("not this wallet"), "{err}");
    }

    #[test]
    fn a_response_carrying_no_burn_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_burns(&[], &wallet, 1_000)
            .expect_err("an action that was never built must be refused");
        assert!(err.to_string().contains("found 0"), "{err}");
    }

    #[test]
    fn a_second_burn_smuggled_alongside_the_first_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_burns(
            &[burn_tx(wallet, 1_000), burn_tx(wallet, 1_000)],
            &wallet,
            1_000,
        )
        .expect_err("a batch burning twice must be refused");
        assert!(err.to_string().contains("found 2"), "{err}");
    }

    #[test]
    fn a_proposal_carrying_a_top_level_burn_is_refused() {
        let err = assert_wraps_no_burn(&[burn_tx(Pubkey::new_unique(), 1_000)])
            .expect_err("a proposal must wrap the burn, not carry it");
        assert!(err.to_string().contains("top level"), "{err}");
    }
}
