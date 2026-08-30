use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::{
    blockchain_api::types::DcDelegateRequest, dao::SubDao, keypair::Signer, programs::KnownProgram,
    transaction::VersionedTransaction, verify,
};

/// `delegate_data_credits_v0` names the sub-dao being delegated to at account
/// index 4, and carries the router key and amount in its arguments.
const DELEGATE_METHODS: &[&str] = &["delegate_data_credits_v0"];
const SUB_DAO_ACCOUNT_INDEX: usize = 4;

/// Refuse to sign unless the delegation sends the DC that was asked for, to the
/// router and sub-dao that were asked for.
///
/// The router key is an argument rather than an account, so a substituted one
/// resolves to a different escrow: the DC leaves this wallet and is spendable
/// by someone else's router, and delegation is not reversible from here.
fn assert_delegates_to(
    unsigned: &[VersionedTransaction],
    subdao: SubDao,
    router_key: &str,
    amount: u64,
) -> Result<()> {
    let found = verify::find_methods(unsigned, KnownProgram::DataCredits, DELEGATE_METHODS)?;
    let [delegate] = found.as_slice() else {
        bail!(
            "expected exactly one data-credits delegation, found {}; refusing to sign",
            found.len()
        );
    };
    let sub_dao = delegate.account(SUB_DAO_ACCOUNT_INDEX)?;
    if sub_dao != subdao.key() {
        bail!("the delegation is to sub-dao {sub_dao}, not the requested {subdao}");
    }
    // The IDL declares one `args` parameter, so the fields sit a level down.
    let args = delegate
        .args()
        .ok_or_else(|| anyhow!("the delegation's arguments could not be read; refusing to sign"))?;
    let args = &args["args"];
    let got_router = args["router_key"]
        .as_str()
        .ok_or_else(|| anyhow!("the delegation names no router key; refusing to sign"))?;
    if got_router != router_key {
        bail!("the delegation is to router {got_router}, not the requested {router_key}");
    }
    let got_amount = args["amount"]
        .as_u64()
        .ok_or_else(|| anyhow!("the delegation names no amount; refusing to sign"))?;
    if got_amount != amount {
        bail!("the delegation moves {got_amount} DC, not the requested {amount}");
    }
    Ok(())
}

/// A proposal delegates the vault's DC through a Squads instruction this cannot
/// read, so it is held to carrying no delegation of its own.
fn assert_wraps_no_delegation(unsigned: &[VersionedTransaction]) -> Result<()> {
    if !verify::find_methods(unsigned, KnownProgram::DataCredits, DELEGATE_METHODS)?.is_empty() {
        bail!("a Squads proposal must not delegate DC at the top level; refusing to sign");
    }
    Ok(())
}

#[derive(Debug, Clone, clap::Args)]
/// Delegate DC from this wallet to a given router
pub struct Cmd {
    /// Subdao to delegate DC to
    subdao: SubDao,

    /// Public Helium payer key to delegate to
    payer: String,

    /// Amount of DC to delegate
    dc: u64,

    /// The DC is sourced from the resolved vault's DC ATA.
    #[command(flatten)]
    squads: SquadsOpts,

    /// Commit the delegation
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;

        let client = opts.client()?;

        // In propose mode the memo rides on the Squads proposal; a direct
        // delegate sends no in-tx memo (matching prior behavior).
        let (multisig, memo) = self.squads.resolve(&client, &signer.pubkey()).await?;
        let is_proposal = multisig.is_some();

        let api = opts.blockchain_api()?;
        let response = api
            .dc_delegate(&DcDelegateRequest {
                owner: signer.pubkey().to_string(),
                router_key: self.payer.clone(),
                amount: self.dc.to_string(),
                mint: self.subdao.token().mint().to_string(),
                memo,
                multisig,
            })
            .await?;
        let unsigned = response.decode_transactions()?;
        if is_proposal {
            assert_wraps_no_delegation(&unsigned)?;
        } else {
            assert_delegates_to(&unsigned, self.subdao, &self.payer, self.dc)?;
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
    use helium_lib::keypair::Pubkey;
    use helium_lib::solana_sdk::{
        instruction::{AccountMeta, Instruction},
        message::{Message, VersionedMessage},
    };

    /// `sha256("global:delegate_data_credits_v0")[..8]`.
    const DELEGATE_DISCRIMINATOR: [u8; 8] = [154, 56, 226, 128, 162, 115, 226, 5];

    const ROUTER: &str = "13WvV82S7QN3VMzMSieiGxvuaPKknMtf213E5JmjeWTZ2wDHmYm";

    /// A `delegate_data_credits_v0` with the account layout and borsh argument
    /// encoding the IDL declares, so what the guard reads here is what it would
    /// read on chain.
    fn delegate_tx(subdao: SubDao, router_key: &str, amount: u64) -> VersionedTransaction {
        let mut data = DELEGATE_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&(router_key.len() as u32).to_le_bytes());
        data.extend_from_slice(router_key.as_bytes());

        let payer = Pubkey::new_unique();
        let accounts = [
            Pubkey::new_unique(), // delegated_data_credits
            Pubkey::new_unique(), // data_credits
            Pubkey::new_unique(), // dc_mint
            Pubkey::new_unique(), // dao
            subdao.key(),
            payer, // owner
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
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&payer))),
        }
    }

    #[test]
    fn the_requested_delegation_is_accepted() {
        assert_delegates_to(
            &[delegate_tx(SubDao::Iot, ROUTER, 5_000)],
            SubDao::Iot,
            ROUTER,
            5_000,
        )
        .expect("the requested delegation");
    }

    #[test]
    fn a_delegation_to_another_router_is_refused() {
        let other = "13M8dUbxymE3xtiAXszRkGMmezMhBS8Li7wEsMojLdb4Sdxc4wc";
        let err = assert_delegates_to(
            &[delegate_tx(SubDao::Iot, other, 5_000)],
            SubDao::Iot,
            ROUTER,
            5_000,
        )
        .expect_err("a substituted router must be refused");
        assert!(err.to_string().contains("not the requested"), "{err}");
    }

    #[test]
    fn a_delegation_of_another_amount_is_refused() {
        let err = assert_delegates_to(
            &[delegate_tx(SubDao::Iot, ROUTER, 9_999)],
            SubDao::Iot,
            ROUTER,
            5_000,
        )
        .expect_err("a substituted amount must be refused");
        assert!(err.to_string().contains("9999"), "{err}");
    }

    #[test]
    fn a_delegation_to_another_subdao_is_refused() {
        let err = assert_delegates_to(
            &[delegate_tx(SubDao::Mobile, ROUTER, 5_000)],
            SubDao::Iot,
            ROUTER,
            5_000,
        )
        .expect_err("a substituted sub-dao must be refused");
        assert!(err.to_string().contains("sub-dao"), "{err}");
    }

    #[test]
    fn a_response_carrying_no_delegation_is_refused() {
        let err = assert_delegates_to(&[], SubDao::Iot, ROUTER, 5_000)
            .expect_err("an action that was never built must be refused");
        assert!(err.to_string().contains("found 0"), "{err}");
    }

    #[test]
    fn a_second_delegation_smuggled_alongside_the_first_is_refused() {
        let err = assert_delegates_to(
            &[
                delegate_tx(SubDao::Iot, ROUTER, 5_000),
                delegate_tx(SubDao::Iot, ROUTER, 5_000),
            ],
            SubDao::Iot,
            ROUTER,
            5_000,
        )
        .expect_err("a batch delegating twice must be refused");
        assert!(err.to_string().contains("found 2"), "{err}");
    }

    #[test]
    fn a_delegation_whose_arguments_do_not_decode_is_refused_as_unreadable() {
        // Asserting which refusal fires, not just that one did: arguments that
        // cannot be read are a different fault from arguments that name the
        // wrong router, and the two must not collapse into one message.
        let payer = Pubkey::new_unique();
        let mut data = DELEGATE_DISCRIMINATOR.to_vec();
        data.extend_from_slice(&[0, 1, 2]); // too short for the u64 amount
                                            // Every other check must pass, or the refusal under test never runs.
        let accounts = [
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            SubDao::Iot.key(),
            payer,
        ];
        let ix = Instruction {
            program_id: KnownProgram::DataCredits.id(),
            accounts: accounts
                .iter()
                .map(|key| AccountMeta::new(*key, false))
                .collect(),
            data,
        };
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&payer))),
        };
        let err = assert_delegates_to(&[tx], SubDao::Iot, ROUTER, 5_000)
            .expect_err("an undecodable delegation must be refused");
        assert!(err.to_string().contains("could not be read"), "{err}");
    }

    #[test]
    fn a_proposal_carrying_a_top_level_delegation_is_refused() {
        let err = assert_wraps_no_delegation(&[delegate_tx(SubDao::Iot, ROUTER, 5_000)])
            .expect_err("a proposal must wrap the delegation, not carry it");
        assert!(err.to_string().contains("top level"), "{err}");
    }
}
