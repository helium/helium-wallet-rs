use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::{
    blockchain_api::types::TransferHotspotRequest,
    entity_key,
    keypair::{Pubkey, Signer},
    programs::KnownProgram,
    transaction::VersionedTransaction,
    verify,
};

/// Bubblegum's `transfer` names the asset's current owner at account index 1
/// and the wallet it is being handed to at index 3.
const TRANSFER_METHODS: &[&str] = &["transfer"];
const LEAF_OWNER_INDEX: usize = 1;
const NEW_LEAF_OWNER_INDEX: usize = 3;

/// Refuse to sign unless the transaction hands the asset to the wallet that was
/// asked for, on this wallet's own authority.
///
/// The action summary names the program rather than the destination, so a
/// substituted recipient reads exactly like the requested one; the transfer
/// cannot be undone without the new owner's cooperation.
fn assert_transfers_asset(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    recipient: &Pubkey,
) -> Result<()> {
    let found = verify::find_methods(unsigned, KnownProgram::Bubblegum, TRANSFER_METHODS)?;
    let [transfer] = found.as_slice() else {
        bail!(
            "expected exactly one asset transfer, found {}; refusing to sign",
            found.len()
        );
    };
    let new_owner = transfer.account(NEW_LEAF_OWNER_INDEX)?;
    if new_owner != *recipient {
        bail!("the transfer would hand the asset to {new_owner}, not the requested {recipient}");
    }
    let owner = transfer.account(LEAF_OWNER_INDEX)?;
    if owner != *wallet {
        bail!("the transfer moves an asset owned by {owner}, not this wallet");
    }
    Ok(())
}

/// A proposal moves the asset from the multisig's vault, wrapped in a Squads
/// instruction this cannot read, so it is held instead to transferring nothing
/// at the top level: without that, a plain transfer returned in place of a
/// proposal would move this wallet's own asset unverified.
fn assert_wraps_no_asset_transfer(unsigned: &[VersionedTransaction]) -> Result<()> {
    if !verify::find_methods(unsigned, KnownProgram::Bubblegum, TRANSFER_METHODS)?.is_empty() {
        bail!("a Squads proposal must not transfer an asset at the top level; refusing to sign");
    }
    Ok(())
}

#[derive(Clone, Debug, clap::Args)]
/// Transfer an asset (NFT) to another owner
pub struct Cmd {
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,

    /// Solana address of the recipient of the asset
    pub recipient: Pubkey,
    /// Submit as a Squads v4 proposal.
    /// The asset's current owner must be the resolved vault.
    #[command(flatten)]
    pub squads: SquadsOpts,
    /// Commit the transfer
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;

        // With `--squads` the transfer is built from the multisig's vault (which
        // must own the asset) and wrapped as a proposal; otherwise it transfers
        // from this wallet. Both build via the API.
        let (multisig, memo) = self.squads.resolve(&client, &signer.pubkey()).await?;
        let is_proposal = multisig.is_some();

        let api = opts.blockchain_api()?;
        let response = api
            .transfer_hotspot(&TransferHotspotRequest {
                wallet_address: signer.pubkey().to_string(),
                hotspot_pubkey: self.entity_key.to_string(),
                recipient: self.recipient.to_string(),
                multisig,
                memo,
            })
            .await?;
        let unsigned = response.decode_transactions()?;
        if is_proposal {
            assert_wraps_no_asset_transfer(&unsigned)?;
        } else {
            assert_transfers_asset(&unsigned, &signer.pubkey(), &self.recipient)?;
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
    use helium_lib::solana_sdk::{
        instruction::{AccountMeta, Instruction},
        message::{Message, VersionedMessage},
    };

    /// Bubblegum's `transfer` discriminator, `sha256("global:transfer")[..8]`.
    const TRANSFER_DISCRIMINATOR: [u8; 8] = [163, 52, 200, 231, 140, 3, 69, 186];

    /// A bubblegum `transfer` with the account layout the IDL declares, so the
    /// indices the guard reads mean here what they mean on chain.
    fn transfer_tx(leaf_owner: Pubkey, new_leaf_owner: Pubkey) -> VersionedTransaction {
        anchor_tx(
            KnownProgram::Bubblegum.id(),
            TRANSFER_DISCRIMINATOR,
            &[
                Pubkey::new_unique(), // tree_authority
                leaf_owner,
                leaf_owner,           // leaf_delegate
                new_leaf_owner,       // the account the guard checks
                Pubkey::new_unique(), // merkle_tree
                Pubkey::new_unique(), // log_wrapper
                Pubkey::new_unique(), // compression_program
                Pubkey::new_unique(), // system_program
            ],
        )
    }

    fn anchor_tx(
        program: Pubkey,
        discriminator: [u8; 8],
        accounts: &[Pubkey],
    ) -> VersionedTransaction {
        let payer = accounts[1];
        let ix = Instruction {
            program_id: program,
            accounts: accounts
                .iter()
                .map(|key| AccountMeta::new(*key, false))
                .collect(),
            data: discriminator.to_vec(),
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&payer))),
        }
    }

    #[test]
    fn a_transfer_to_the_requested_recipient_is_accepted() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        assert_transfers_asset(&[transfer_tx(wallet, recipient)], &wallet, &recipient)
            .expect("the requested transfer");
    }

    #[test]
    fn a_transfer_to_another_wallet_is_refused() {
        let wallet = Pubkey::new_unique();
        let requested = Pubkey::new_unique();
        let attacker = Pubkey::new_unique();
        let err = assert_transfers_asset(&[transfer_tx(wallet, attacker)], &wallet, &requested)
            .expect_err("a substituted recipient must be refused");
        assert!(err.to_string().contains("not the requested"), "{err}");
    }

    #[test]
    fn a_transfer_of_someone_elses_asset_is_refused() {
        let wallet = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let err = assert_transfers_asset(&[transfer_tx(other, recipient)], &wallet, &recipient)
            .expect_err("an asset this wallet does not own must be refused");
        assert!(err.to_string().contains("not this wallet"), "{err}");
    }

    #[test]
    fn a_response_carrying_no_transfer_is_refused() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let err = assert_transfers_asset(&[], &wallet, &recipient)
            .expect_err("an action that was never built must be refused");
        assert!(err.to_string().contains("found 0"), "{err}");
    }

    #[test]
    fn a_second_transfer_smuggled_alongside_the_first_is_refused() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let attacker = Pubkey::new_unique();
        let err = assert_transfers_asset(
            &[
                transfer_tx(wallet, recipient),
                transfer_tx(wallet, attacker),
            ],
            &wallet,
            &recipient,
        )
        .expect_err("a batch moving a second asset must be refused");
        assert!(err.to_string().contains("found 2"), "{err}");
    }

    #[test]
    fn a_proposal_carrying_a_top_level_transfer_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_wraps_no_asset_transfer(&[transfer_tx(wallet, Pubkey::new_unique())])
            .expect_err("a proposal must wrap the transfer, not carry it");
        assert!(err.to_string().contains("top level"), "{err}");
    }

    #[test]
    fn a_proposal_that_wraps_its_transfer_is_accepted() {
        assert_wraps_no_asset_transfer(&[anchor_tx(
            KnownProgram::SquadsV4.id(),
            [1, 2, 3, 4, 5, 6, 7, 8],
            &[Pubkey::new_unique(), Pubkey::new_unique()],
        )])
        .expect("a wrapped transfer");
    }
}
