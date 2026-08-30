use crate::cmd::{squads::SquadsOpts, *};
use helium_lib::{
    blockchain_api::types::HotspotBurnRequest, entity_key, keypair::Signer, programs::KnownProgram,
    transaction::VersionedTransaction, verify,
};

/// Bubblegum's `burn` names the asset's owner at account index 1.
const BURN_METHODS: &[&str] = &["burn"];
const LEAF_OWNER_INDEX: usize = 1;

/// Refuse to sign unless the transaction burns one asset, owned by this wallet.
///
/// Which asset is not checked here: the burn identifies its leaf by a merkle
/// proof over the tree, and resolving the entity key to that leaf is the lookup
/// the request already asked the service to do. So this catches a batch that
/// burns more than the one asset asked for, and a burn of an asset held by
/// someone else, and does not catch a different asset of this wallet's.
fn assert_burns_one_owned_asset(unsigned: &[VersionedTransaction], wallet: &Pubkey) -> Result<()> {
    let found = verify::find_methods(unsigned, KnownProgram::Bubblegum, BURN_METHODS)?;
    let [burn] = found.as_slice() else {
        bail!(
            "expected exactly one asset burn, found {}; refusing to sign",
            found.len()
        );
    };
    let owner = burn.account(LEAF_OWNER_INDEX)?;
    if owner != *wallet {
        bail!("the burn destroys an asset owned by {owner}, not this wallet");
    }
    Ok(())
}

/// A proposal burns the vault's asset inside a Squads instruction this cannot
/// read, so it is held to burning nothing of its own.
fn assert_wraps_no_burn(unsigned: &[VersionedTransaction]) -> Result<()> {
    if !verify::find_methods(unsigned, KnownProgram::Bubblegum, BURN_METHODS)?.is_empty() {
        bail!("a Squads proposal must not burn an asset at the top level; refusing to sign");
    }
    Ok(())
}

#[derive(Clone, Debug, clap::Args)]
/// Burn a given asset (NFT)
pub struct Cmd {
    /// Entity key of asset to burn
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    /// Submit as a Squads v4 proposal.
    /// The asset's current owner must be the resolved vault.
    #[command(flatten)]
    pub squads: SquadsOpts,
    /// Commit the transaction
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let signer = opts.load_signer()?;

        // With `--squads` the API builds the burn from the multisig's vault
        // (which must own the asset) and wraps it as a proposal; otherwise the
        // wallet burns its own asset directly.
        let (multisig, memo) = self.squads.resolve(&client, &signer.pubkey()).await?;
        let is_proposal = multisig.is_some();

        let api = opts.blockchain_api()?;
        let response = api
            .burn_hotspot(&HotspotBurnRequest {
                wallet_address: signer.pubkey().to_string(),
                hotspot_pubkey: self.entity_key.to_string(),
                multisig,
                memo,
            })
            .await?;
        let unsigned = response.decode_transactions()?;
        if is_proposal {
            assert_wraps_no_burn(&unsigned)?;
        } else {
            assert_burns_one_owned_asset(&unsigned, &signer.pubkey())?;
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

    /// `sha256("global:burn")[..8]`, as the bubblegum IDL declares it.
    const BURN_DISCRIMINATOR: [u8; 8] = [116, 110, 29, 56, 107, 219, 42, 93];

    fn burn_tx(leaf_owner: Pubkey) -> VersionedTransaction {
        let accounts = [
            Pubkey::new_unique(), // tree_authority
            leaf_owner,
            leaf_owner,           // leaf_delegate
            Pubkey::new_unique(), // merkle_tree
        ];
        let ix = Instruction {
            program_id: KnownProgram::Bubblegum.id(),
            accounts: accounts
                .iter()
                .map(|key| AccountMeta::new(*key, false))
                .collect(),
            data: BURN_DISCRIMINATOR.to_vec(),
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&leaf_owner))),
        }
    }

    #[test]
    fn a_burn_of_this_wallets_asset_is_accepted() {
        let wallet = Pubkey::new_unique();
        assert_burns_one_owned_asset(&[burn_tx(wallet)], &wallet).expect("an owned asset");
    }

    #[test]
    fn a_burn_of_someone_elses_asset_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_burns_one_owned_asset(&[burn_tx(Pubkey::new_unique())], &wallet)
            .expect_err("an asset this wallet does not own must be refused");
        assert!(err.to_string().contains("not this wallet"), "{err}");
    }

    #[test]
    fn a_batch_burning_more_than_one_asset_is_refused() {
        // The check this guard is actually for: which asset is not verifiable
        // from the instruction, but how many are is.
        let wallet = Pubkey::new_unique();
        let err = assert_burns_one_owned_asset(&[burn_tx(wallet), burn_tx(wallet)], &wallet)
            .expect_err("a batch burning two assets must be refused");
        assert!(err.to_string().contains("found 2"), "{err}");
    }

    #[test]
    fn a_response_carrying_no_burn_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_burns_one_owned_asset(&[], &wallet)
            .expect_err("an action that was never built must be refused");
        assert!(err.to_string().contains("found 0"), "{err}");
    }

    #[test]
    fn a_proposal_carrying_a_top_level_burn_is_refused() {
        let err = assert_wraps_no_burn(&[burn_tx(Pubkey::new_unique())])
            .expect_err("a proposal must wrap the burn, not carry it");
        assert!(err.to_string().contains("top level"), "{err}");
    }
}
