//! What a built transaction has to do to be the action that was asked for.
//!
//! Each check here answers one question: does this transaction carry the action
//! a caller requested, once, naming the accounts and values the caller named?
//! The account positions come from the IDLs this crate ships, so a caller does
//! not have to know a program's layout to hold a transaction to its own intent.
//!
//! These are for a caller that asked a remote builder for an action and is
//! about to authorize the result -- either by signing it, or by serving it to
//! someone else who will.

use super::{find_methods, NamedInstruction, VerifyError};
use crate::{hotspot, keypair::Pubkey, programs::KnownProgram, transaction::VersionedTransaction};

/// Bubblegum's `transfer` and `burn` name the asset's current owner at account
/// index 1; `transfer` names the wallet it is handed to at index 3.
const ASSET_TRANSFER: &[&str] = &["transfer"];
const ASSET_BURN: &[&str] = &["burn"];
const LEAF_OWNER_INDEX: usize = 1;
const NEW_LEAF_OWNER_INDEX: usize = 3;

/// `mint_data_credits_v0` names the wallet credited at index 4 and the wallet
/// whose HNT pays for it at index 5.
const DC_MINT: &[&str] = &["mint_data_credits_v0"];
const DC_RECIPIENT_INDEX: usize = 4;
const DC_MINT_OWNER_INDEX: usize = 5;

/// `delegate_data_credits_v0` names the sub-dao at index 4 and carries the
/// router key and amount in its arguments.
const DC_DELEGATE: &[&str] = &["delegate_data_credits_v0"];
const DC_SUB_DAO_INDEX: usize = 4;

/// `burn_without_tracking_v0` declares its accounts as one composite, which
/// Anchor flattens into the instruction; the owner is third.
const DC_BURN: &[&str] = &["burn_without_tracking_v0"];
const DC_BURN_OWNER_INDEX: usize = 2;

/// Both reward-destination updates put the new destination at index 2 and the
/// owner-signer at index 1.
const REWARDS_DESTINATION: &[&str] =
    &["update_destination_v0", "update_compression_destination_v0"];
const DESTINATION_INDEX: usize = 2;
const DESTINATION_OWNER_INDEX: usize = 1;

/// Both info updates and both data-only onboards name the hotspot's owner at
/// index 3 and carry the asserted location in their arguments.
const HOTSPOT_UPDATE: &[&str] = &["update_iot_info_v0", "update_mobile_info_v0"];
const HOTSPOT_ONBOARD: &[&str] = &[
    "onboard_data_only_iot_hotspot_v0",
    "onboard_data_only_mobile_hotspot_v0",
];
const HOTSPOT_OWNER_INDEX: usize = 3;

/// `issue_data_only_entity_v0` names the wallet the hotspot is minted to at
/// index 10.
const HOTSPOT_ISSUE: &[&str] = &["issue_data_only_entity_v0"];
const ISSUE_RECIPIENT_INDEX: usize = 10;

/// The one instruction across `txs` invoking `program` by one of `methods`.
///
/// Zero and many are both refused: an action that was never built must not read
/// the same as one built correctly, and a second copy alongside the first is a
/// second action the caller did not ask for.
fn sole<'a>(
    txs: &'a [VersionedTransaction],
    program: KnownProgram,
    methods: &[&str],
    action: &'static str,
) -> Result<NamedInstruction<'a>, VerifyError> {
    let mut found = find_methods(txs, program, methods)?;
    if found.len() != 1 {
        return Err(VerifyError::ActionCount {
            action,
            found: found.len(),
        });
    }
    Ok(found.remove(0))
}

/// Refuse unless the account `ix` names at `index` is `expected`.
fn account_is(
    ix: &NamedInstruction<'_>,
    index: usize,
    expected: &Pubkey,
    action: &'static str,
    role: &'static str,
) -> Result<(), VerifyError> {
    let actual = ix.account(index)?;
    if actual != *expected {
        return Err(VerifyError::ActionAccount {
            action,
            role,
            expected: *expected,
            actual,
        });
    }
    Ok(())
}

/// The decoded arguments of `ix`, under the single `args` parameter the HPL
/// IDLs declare.
fn args(ix: &NamedInstruction<'_>, action: &'static str) -> Result<serde_json::Value, VerifyError> {
    ix.args()
        .map(|args| args["args"].clone())
        .ok_or(VerifyError::ActionUnreadable { action })
}

/// Refuse unless the transaction hands one asset to `recipient`, on `owner`'s
/// authority.
pub fn assert_asset_transfer(
    txs: &[VersionedTransaction],
    owner: &Pubkey,
    recipient: &Pubkey,
) -> Result<(), VerifyError> {
    const ACTION: &str = "asset transfer";
    let ix = sole(txs, KnownProgram::Bubblegum, ASSET_TRANSFER, ACTION)?;
    account_is(&ix, NEW_LEAF_OWNER_INDEX, recipient, ACTION, "recipient")?;
    account_is(&ix, LEAF_OWNER_INDEX, owner, ACTION, "current owner")
}

/// Refuse unless the transaction burns one asset held by `owner`.
///
/// Which asset is not checked: the burn names its leaf by a merkle proof over
/// the tree, and resolving an entity key to that leaf is the lookup the request
/// already delegated. So this refuses a batch burning more than the one asset
/// asked for, and a burn of an asset held by someone else, and does not refuse a
/// different asset of `owner`'s.
pub fn assert_asset_burn(txs: &[VersionedTransaction], owner: &Pubkey) -> Result<(), VerifyError> {
    const ACTION: &str = "asset burn";
    let ix = sole(txs, KnownProgram::Bubblegum, ASSET_BURN, ACTION)?;
    account_is(&ix, LEAF_OWNER_INDEX, owner, ACTION, "owner")
}

/// Refuse unless the transaction credits DC to `recipient`, paid for by
/// `owner`'s HNT.
///
/// DC is non-transferable once minted, so a substituted recipient is not
/// recoverable: the HNT burns either way and the credits land elsewhere for
/// good.
pub fn assert_dc_mint(
    txs: &[VersionedTransaction],
    owner: &Pubkey,
    recipient: &Pubkey,
) -> Result<(), VerifyError> {
    const ACTION: &str = "data-credits mint";
    let ix = sole(txs, KnownProgram::DataCredits, DC_MINT, ACTION)?;
    account_is(&ix, DC_RECIPIENT_INDEX, recipient, ACTION, "recipient")?;
    account_is(&ix, DC_MINT_OWNER_INDEX, owner, ACTION, "payer")
}

/// Refuse unless the transaction delegates `amount` DC to `router_key` on
/// `sub_dao`.
///
/// The router key is an argument rather than an account, so a substituted one
/// resolves to a different escrow: the DC leaves the wallet and becomes
/// spendable by someone else's router, and nothing here reverses it.
pub fn assert_dc_delegate(
    txs: &[VersionedTransaction],
    sub_dao: &Pubkey,
    router_key: &str,
    amount: u64,
) -> Result<(), VerifyError> {
    const ACTION: &str = "data-credits delegation";
    let ix = sole(txs, KnownProgram::DataCredits, DC_DELEGATE, ACTION)?;
    account_is(&ix, DC_SUB_DAO_INDEX, sub_dao, ACTION, "sub-dao")?;
    let args = args(&ix, ACTION)?;
    let actual = args["router_key"]
        .as_str()
        .ok_or(VerifyError::ActionUnreadable { action: ACTION })?;
    if actual != router_key {
        return Err(VerifyError::ActionValue {
            action: ACTION,
            field: "router key",
            expected: router_key.to_string(),
            actual: actual.to_string(),
        });
    }
    assert_arg_amount(&args, amount, ACTION)
}

/// Refuse unless the transaction burns `amount` DC held by `owner`.
pub fn assert_dc_burn(
    txs: &[VersionedTransaction],
    owner: &Pubkey,
    amount: u64,
) -> Result<(), VerifyError> {
    const ACTION: &str = "data-credits burn";
    let ix = sole(txs, KnownProgram::DataCredits, DC_BURN, ACTION)?;
    account_is(&ix, DC_BURN_OWNER_INDEX, owner, ACTION, "holder")?;
    assert_arg_amount(&args(&ix, ACTION)?, amount, ACTION)
}

fn assert_arg_amount(
    args: &serde_json::Value,
    expected: u64,
    action: &'static str,
) -> Result<(), VerifyError> {
    let actual = args["amount"]
        .as_u64()
        .ok_or(VerifyError::ActionUnreadable { action })?;
    if actual != expected {
        return Err(VerifyError::ActionValue {
            action,
            field: "amount",
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}

/// Refuse unless the transaction points a hotspot's rewards at `destination`,
/// on `owner`'s authority.
///
/// A substituted destination is the highest-value thing a builder can return:
/// unlike a one-off transfer it is a standing redirect that keeps paying after
/// the compromise is found, and undoing it needs a second on-chain action.
pub fn assert_rewards_destination(
    txs: &[VersionedTransaction],
    owner: &Pubkey,
    destination: &Pubkey,
) -> Result<(), VerifyError> {
    const ACTION: &str = "rewards-destination update";
    let ix = sole(
        txs,
        KnownProgram::LazyDistributor,
        REWARDS_DESTINATION,
        ACTION,
    )?;
    account_is(&ix, DESTINATION_INDEX, destination, ACTION, "destination")?;
    account_is(&ix, DESTINATION_OWNER_INDEX, owner, ACTION, "authority")
}

/// Refuse unless the transaction mints one hotspot to `recipient`.
///
/// The recipient is the whole of what issuing decides, and the add-gateway
/// token it is minted from cannot be spent twice to correct it.
pub fn assert_hotspot_issue(
    txs: &[VersionedTransaction],
    recipient: &Pubkey,
) -> Result<(), VerifyError> {
    const ACTION: &str = "hotspot issue";
    let ix = sole(
        txs,
        KnownProgram::HeliumEntityManager,
        HOTSPOT_ISSUE,
        ACTION,
    )?;
    account_is(&ix, ISSUE_RECIPIENT_INDEX, recipient, ACTION, "recipient")
}

/// Refuse unless the transaction onboards a hotspot to `owner` at the location
/// `lat`/`lon` name.
pub fn assert_hotspot_onboard(
    txs: &[VersionedTransaction],
    owner: &Pubkey,
    lat: Option<f64>,
    lon: Option<f64>,
) -> Result<(), VerifyError> {
    const ACTION: &str = "hotspot onboard";
    let ix = sole(
        txs,
        KnownProgram::HeliumEntityManager,
        HOTSPOT_ONBOARD,
        ACTION,
    )?;
    account_is(&ix, HOTSPOT_OWNER_INDEX, owner, ACTION, "owner")?;
    assert_asserted_location(&args(&ix, ACTION)?, lat, lon, ACTION)
}

/// Refuse unless the transaction updates a hotspot owned by `owner` to the
/// location `lat`/`lon` name, and sets nothing else the caller did not ask for.
///
/// Location is compared exactly, from the coordinates the caller sent. Gain and
/// elevation are checked only for presence: their unit conversion belongs to
/// whoever built the transaction, and a second copy of its rounding here would
/// refuse honest updates the day it changes.
pub fn assert_hotspot_update(
    txs: &[VersionedTransaction],
    owner: &Pubkey,
    lat: Option<f64>,
    lon: Option<f64>,
    gain_requested: bool,
    elevation_requested: bool,
) -> Result<(), VerifyError> {
    const ACTION: &str = "hotspot info update";
    let ix = sole(
        txs,
        KnownProgram::HeliumEntityManager,
        HOTSPOT_UPDATE,
        ACTION,
    )?;
    account_is(&ix, HOTSPOT_OWNER_INDEX, owner, ACTION, "owner")?;
    let args = args(&ix, ACTION)?;
    assert_asserted_location(&args, lat, lon, ACTION)?;
    for (field, requested) in [("gain", gain_requested), ("elevation", elevation_requested)] {
        if !requested && !args[field].is_null() {
            return Err(VerifyError::ActionUnrequested {
                action: ACTION,
                field,
            });
        }
    }
    Ok(())
}

fn assert_asserted_location(
    args: &serde_json::Value,
    lat: Option<f64>,
    lon: Option<f64>,
    action: &'static str,
) -> Result<(), VerifyError> {
    let expected = hotspot::cell_for(lat, lon)
        .map_err(|_| VerifyError::ActionUnreadable { action })?
        .map(u64::from);
    let actual = args["location"].as_u64();
    if actual != expected {
        return Err(VerifyError::ActionValue {
            action,
            field: "location",
            expected: format!("{expected:?}"),
            actual: format!("{actual:?}"),
        });
    }
    Ok(())
}

/// Refuse a proposal that carries `methods` at the top level.
///
/// A proposal moves a multisig vault's funds inside an instruction that cannot
/// be read here, so it is held to carrying no such action of its own: without
/// that, a plain action returned in place of a proposal would move the
/// proposer's own funds unverified.
pub fn assert_wraps_no(
    txs: &[VersionedTransaction],
    program: KnownProgram,
    methods: &[&str],
    action: &'static str,
) -> Result<(), VerifyError> {
    if find_methods(txs, program, methods)?.is_empty() {
        return Ok(());
    }
    Err(VerifyError::ActionNotWrapped { action })
}

/// The wrapped-action check for each action a proposal can carry.
pub mod wrapped {
    use super::*;

    macro_rules! wraps_no {
        ($name:ident, $program:expr, $methods:expr, $action:literal) => {
            #[doc = concat!("Refuse a proposal carrying a top-level ", $action, ".")]
            pub fn $name(txs: &[VersionedTransaction]) -> Result<(), VerifyError> {
                assert_wraps_no(txs, $program, $methods, $action)
            }
        };
    }

    wraps_no!(
        asset_transfer,
        KnownProgram::Bubblegum,
        ASSET_TRANSFER,
        "asset transfer"
    );
    wraps_no!(
        asset_burn,
        KnownProgram::Bubblegum,
        ASSET_BURN,
        "asset burn"
    );
    wraps_no!(
        dc_delegate,
        KnownProgram::DataCredits,
        DC_DELEGATE,
        "data-credits delegation"
    );
    wraps_no!(
        dc_burn,
        KnownProgram::DataCredits,
        DC_BURN,
        "data-credits burn"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solana_sdk::{
        instruction::{AccountMeta, Instruction},
        message::{Message, VersionedMessage},
    };

    /// Discriminators as the shipped IDLs declare them. Written out rather than
    /// read from the same IDL the code reads, so a changed discriminator fails
    /// here instead of agreeing with itself.
    const D_ASSET_TRANSFER: [u8; 8] = [163, 52, 200, 231, 140, 3, 69, 186];
    const D_ASSET_BURN: [u8; 8] = [116, 110, 29, 56, 107, 219, 42, 93];
    const D_DC_MINT: [u8; 8] = [78, 109, 169, 132, 144, 94, 221, 57];
    const D_DC_DELEGATE: [u8; 8] = [154, 56, 226, 128, 162, 115, 226, 5];
    const D_DC_BURN: [u8; 8] = [129, 106, 43, 4, 52, 143, 102, 208];
    const D_UPDATE_IOT: [u8; 8] = [211, 235, 205, 29, 109, 86, 153, 39];
    const D_ONBOARD_IOT: [u8; 8] = [98, 179, 127, 51, 58, 191, 174, 188];
    const D_ISSUE: [u8; 8] = [191, 96, 245, 46, 63, 73, 207, 17];
    const D_UPDATE_DESTINATION: [u8; 8] = [196, 237, 208, 178, 104, 7, 36, 14];

    const ROUTER: &str = "13WvV82S7QN3VMzMSieiGxvuaPKknMtf213E5JmjeWTZ2wDHmYm";
    const LAT: f64 = 37.7749;
    const LON: f64 = -122.4194;

    fn tx(
        program: KnownProgram,
        disc: [u8; 8],
        body: &[u8],
        accounts: &[Pubkey],
    ) -> VersionedTransaction {
        let mut data = disc.to_vec();
        data.extend_from_slice(body);
        let ix = Instruction {
            program_id: program.id(),
            accounts: accounts
                .iter()
                .map(|key| AccountMeta::new(*key, false))
                .collect(),
            data,
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&accounts[0]))),
        }
    }

    fn filler(n: usize) -> Vec<Pubkey> {
        (0..n).map(|_| Pubkey::new_unique()).collect()
    }

    fn cell(lat: f64, lon: f64) -> u64 {
        u64::from(
            hotspot::cell_for(Some(lat), Some(lon))
                .expect("a valid coordinate")
                .expect("a cell"),
        )
    }

    fn opt_u64(out: &mut Vec<u8>, v: Option<u64>) {
        match v {
            Some(v) => {
                out.push(1);
                out.extend_from_slice(&v.to_le_bytes());
            }
            None => out.push(0),
        }
    }

    fn opt_i32(out: &mut Vec<u8>, v: Option<i32>) {
        match v {
            Some(v) => {
                out.push(1);
                out.extend_from_slice(&v.to_le_bytes());
            }
            None => out.push(0),
        }
    }

    // --- asset transfer -------------------------------------------------

    fn asset_transfer_tx(owner: Pubkey, recipient: Pubkey) -> VersionedTransaction {
        let mut a = vec![Pubkey::new_unique(), owner, owner, recipient];
        a.extend(filler(4));
        tx(KnownProgram::Bubblegum, D_ASSET_TRANSFER, &[], &a)
    }

    #[test]
    fn an_asset_transfer_to_the_requested_recipient_is_accepted() {
        let (w, r) = (Pubkey::new_unique(), Pubkey::new_unique());
        assert_asset_transfer(&[asset_transfer_tx(w, r)], &w, &r).expect("the requested transfer");
    }

    #[test]
    fn an_asset_transfer_elsewhere_is_refused() {
        let (w, r, bad) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let err = assert_asset_transfer(&[asset_transfer_tx(w, bad)], &w, &r)
            .expect_err("a substituted recipient must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionAccount {
                    role: "recipient",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn an_asset_transfer_of_someone_elses_asset_is_refused() {
        let (w, r) = (Pubkey::new_unique(), Pubkey::new_unique());
        let err = assert_asset_transfer(&[asset_transfer_tx(Pubkey::new_unique(), r)], &w, &r)
            .expect_err("another owner's asset must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionAccount {
                    role: "current owner",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn no_asset_transfer_at_all_is_refused() {
        let (w, r) = (Pubkey::new_unique(), Pubkey::new_unique());
        let err = assert_asset_transfer(&[], &w, &r).expect_err("nothing built must be refused");
        assert!(
            matches!(err, VerifyError::ActionCount { found: 0, .. }),
            "{err}"
        );
    }

    #[test]
    fn a_second_asset_transfer_is_refused() {
        let (w, r) = (Pubkey::new_unique(), Pubkey::new_unique());
        let err =
            assert_asset_transfer(&[asset_transfer_tx(w, r), asset_transfer_tx(w, r)], &w, &r)
                .expect_err("a batch must be refused");
        assert!(
            matches!(err, VerifyError::ActionCount { found: 2, .. }),
            "{err}"
        );
    }

    #[test]
    fn a_proposal_carrying_an_asset_transfer_is_refused() {
        let w = Pubkey::new_unique();
        let err = wrapped::asset_transfer(&[asset_transfer_tx(w, Pubkey::new_unique())])
            .expect_err("a proposal must wrap it");
        assert!(matches!(err, VerifyError::ActionNotWrapped { .. }), "{err}");
        wrapped::asset_transfer(&[]).expect("a proposal that wraps its transfer");
    }

    // --- asset burn -----------------------------------------------------

    fn asset_burn_tx(owner: Pubkey) -> VersionedTransaction {
        let mut a = vec![Pubkey::new_unique(), owner, owner];
        a.extend(filler(4));
        tx(KnownProgram::Bubblegum, D_ASSET_BURN, &[], &a)
    }

    #[test]
    fn an_asset_burn_of_an_owned_asset_is_accepted() {
        let w = Pubkey::new_unique();
        assert_asset_burn(&[asset_burn_tx(w)], &w).expect("an owned asset");
    }

    #[test]
    fn an_asset_burn_of_someone_elses_asset_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_asset_burn(&[asset_burn_tx(Pubkey::new_unique())], &w)
            .expect_err("another owner's asset must be refused");
        assert!(matches!(err, VerifyError::ActionAccount { .. }), "{err}");
    }

    #[test]
    fn a_batch_burning_two_assets_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_asset_burn(&[asset_burn_tx(w), asset_burn_tx(w)], &w)
            .expect_err("a batch must be refused");
        assert!(
            matches!(err, VerifyError::ActionCount { found: 2, .. }),
            "{err}"
        );
    }

    #[test]
    fn a_proposal_carrying_an_asset_burn_is_refused() {
        let err = wrapped::asset_burn(&[asset_burn_tx(Pubkey::new_unique())])
            .expect_err("a proposal must wrap it");
        assert!(matches!(err, VerifyError::ActionNotWrapped { .. }), "{err}");
    }

    // --- dc mint --------------------------------------------------------

    fn dc_mint_tx(recipient: Pubkey, owner: Pubkey) -> VersionedTransaction {
        let mut a = filler(4);
        a.push(recipient);
        a.push(owner);
        a.extend(filler(2));
        tx(KnownProgram::DataCredits, D_DC_MINT, &[], &a)
    }

    #[test]
    fn a_dc_mint_to_the_requested_payee_is_accepted() {
        let (w, p) = (Pubkey::new_unique(), Pubkey::new_unique());
        assert_dc_mint(&[dc_mint_tx(p, w)], &w, &p).expect("the requested mint");
    }

    #[test]
    fn a_dc_mint_credited_elsewhere_is_refused() {
        let (w, p) = (Pubkey::new_unique(), Pubkey::new_unique());
        let err = assert_dc_mint(&[dc_mint_tx(Pubkey::new_unique(), w)], &w, &p)
            .expect_err("a substituted payee must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionAccount {
                    role: "recipient",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn a_dc_mint_paid_by_another_wallet_is_refused() {
        let (w, p) = (Pubkey::new_unique(), Pubkey::new_unique());
        let err = assert_dc_mint(&[dc_mint_tx(p, Pubkey::new_unique())], &w, &p)
            .expect_err("another payer must be refused");
        assert!(
            matches!(err, VerifyError::ActionAccount { role: "payer", .. }),
            "{err}"
        );
    }

    // --- dc delegate ----------------------------------------------------

    fn dc_delegate_tx(sub_dao: Pubkey, router: &str, amount: u64) -> VersionedTransaction {
        let mut body = amount.to_le_bytes().to_vec();
        body.extend_from_slice(&(router.len() as u32).to_le_bytes());
        body.extend_from_slice(router.as_bytes());
        let mut a = filler(4);
        a.push(sub_dao);
        a.extend(filler(1));
        tx(KnownProgram::DataCredits, D_DC_DELEGATE, &body, &a)
    }

    #[test]
    fn the_requested_delegation_is_accepted() {
        let sd = Pubkey::new_unique();
        assert_dc_delegate(&[dc_delegate_tx(sd, ROUTER, 500)], &sd, ROUTER, 500)
            .expect("the requested delegation");
    }

    #[test]
    fn a_delegation_to_another_router_is_refused() {
        let sd = Pubkey::new_unique();
        let other = "13M8dUbxymE3xtiAXszRkGMmezMhBS8Li7wEsMojLdb4Sdxc4wc";
        let err = assert_dc_delegate(&[dc_delegate_tx(sd, other, 500)], &sd, ROUTER, 500)
            .expect_err("a substituted router must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionValue {
                    field: "router key",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn a_delegation_of_another_amount_is_refused() {
        let sd = Pubkey::new_unique();
        let err = assert_dc_delegate(&[dc_delegate_tx(sd, ROUTER, 999)], &sd, ROUTER, 500)
            .expect_err("a substituted amount must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionValue {
                    field: "amount",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn a_delegation_to_another_subdao_is_refused() {
        let err = assert_dc_delegate(
            &[dc_delegate_tx(Pubkey::new_unique(), ROUTER, 500)],
            &Pubkey::new_unique(),
            ROUTER,
            500,
        )
        .expect_err("a substituted sub-dao must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionAccount {
                    role: "sub-dao",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn a_delegation_whose_arguments_do_not_decode_is_refused_as_unreadable() {
        // Every other check passes, so the refusal under test is the one that runs.
        let sd = Pubkey::new_unique();
        let mut a = filler(4);
        a.push(sd);
        a.extend(filler(1));
        let t = tx(KnownProgram::DataCredits, D_DC_DELEGATE, &[0, 1, 2], &a);
        let err = assert_dc_delegate(&[t], &sd, ROUTER, 500)
            .expect_err("an undecodable delegation must be refused");
        assert!(matches!(err, VerifyError::ActionUnreadable { .. }), "{err}");
    }

    // --- dc burn --------------------------------------------------------

    fn dc_burn_tx(owner: Pubkey, amount: u64) -> VersionedTransaction {
        let mut a = filler(2);
        a.push(owner);
        a.extend(filler(1));
        tx(
            KnownProgram::DataCredits,
            D_DC_BURN,
            &amount.to_le_bytes(),
            &a,
        )
    }

    #[test]
    fn the_requested_dc_burn_is_accepted() {
        let w = Pubkey::new_unique();
        assert_dc_burn(&[dc_burn_tx(w, 1_000)], &w, 1_000).expect("the requested burn");
    }

    #[test]
    fn a_dc_burn_of_another_amount_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_dc_burn(&[dc_burn_tx(w, 9_999)], &w, 1_000)
            .expect_err("a substituted amount must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionValue {
                    field: "amount",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn a_dc_burn_of_another_wallets_credits_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_dc_burn(&[dc_burn_tx(Pubkey::new_unique(), 1_000)], &w, 1_000)
            .expect_err("another holder must be refused");
        assert!(
            matches!(err, VerifyError::ActionAccount { role: "holder", .. }),
            "{err}"
        );
    }

    #[test]
    fn a_proposal_carrying_a_dc_burn_or_delegation_is_refused() {
        let w = Pubkey::new_unique();
        assert!(wrapped::dc_burn(&[dc_burn_tx(w, 1)]).is_err());
        assert!(wrapped::dc_delegate(&[dc_delegate_tx(w, ROUTER, 1)]).is_err());
    }

    // --- rewards destination --------------------------------------------

    fn destination_tx(owner: Pubkey, destination: Pubkey) -> VersionedTransaction {
        let a = vec![Pubkey::new_unique(), owner, destination];
        tx(KnownProgram::LazyDistributor, D_UPDATE_DESTINATION, &[], &a)
    }

    #[test]
    fn the_requested_rewards_destination_is_accepted() {
        let (w, d) = (Pubkey::new_unique(), Pubkey::new_unique());
        assert_rewards_destination(&[destination_tx(w, d)], &w, &d)
            .expect("the requested redirect");
    }

    #[test]
    fn a_redirected_rewards_destination_is_refused() {
        let (w, d) = (Pubkey::new_unique(), Pubkey::new_unique());
        let err = assert_rewards_destination(&[destination_tx(w, Pubkey::new_unique())], &w, &d)
            .expect_err("a substituted destination must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionAccount {
                    role: "destination",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn a_rewards_redirect_authorized_by_another_wallet_is_refused() {
        let (w, d) = (Pubkey::new_unique(), Pubkey::new_unique());
        let err = assert_rewards_destination(&[destination_tx(Pubkey::new_unique(), d)], &w, &d)
            .expect_err("another authority must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionAccount {
                    role: "authority",
                    ..
                }
            ),
            "{err}"
        );
    }

    // --- hotspot issue / onboard / update -------------------------------

    fn issue_tx(recipient: Pubkey) -> VersionedTransaction {
        let mut a = filler(10);
        a.push(recipient);
        tx(KnownProgram::HeliumEntityManager, D_ISSUE, &[], &a)
    }

    #[test]
    fn a_hotspot_issued_to_this_wallet_is_accepted() {
        let w = Pubkey::new_unique();
        assert_hotspot_issue(&[issue_tx(w)], &w).expect("issued here");
    }

    #[test]
    fn a_hotspot_issued_elsewhere_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_hotspot_issue(&[issue_tx(Pubkey::new_unique())], &w)
            .expect_err("a substituted recipient must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionAccount {
                    role: "recipient",
                    ..
                }
            ),
            "{err}"
        );
    }

    fn onboard_tx(owner: Pubkey, location: Option<u64>) -> VersionedTransaction {
        let mut body = vec![0u8; 32 * 3];
        body.extend_from_slice(&0u32.to_le_bytes());
        opt_u64(&mut body, location);
        body.push(0); // elevation
        body.push(0); // gain
        let mut a = filler(3);
        a.push(owner);
        tx(KnownProgram::HeliumEntityManager, D_ONBOARD_IOT, &body, &a)
    }

    #[test]
    fn an_onboard_at_the_asserted_location_is_accepted() {
        let w = Pubkey::new_unique();
        assert_hotspot_onboard(
            &[onboard_tx(w, Some(cell(LAT, LON)))],
            &w,
            Some(LAT),
            Some(LON),
        )
        .expect("the asserted location");
    }

    #[test]
    fn an_onboard_at_another_location_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_hotspot_onboard(
            &[onboard_tx(w, Some(cell(51.5074, -0.1278)))],
            &w,
            Some(LAT),
            Some(LON),
        )
        .expect_err("a substituted location must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionValue {
                    field: "location",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn an_onboard_for_another_owner_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_hotspot_onboard(
            &[onboard_tx(Pubkey::new_unique(), Some(cell(LAT, LON)))],
            &w,
            Some(LAT),
            Some(LON),
        )
        .expect_err("another owner must be refused");
        assert!(
            matches!(err, VerifyError::ActionAccount { role: "owner", .. }),
            "{err}"
        );
    }

    #[test]
    fn an_onboard_asserting_an_unrequested_location_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_hotspot_onboard(&[onboard_tx(w, Some(cell(LAT, LON)))], &w, None, None)
            .expect_err("an unrequested assertion must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionValue {
                    field: "location",
                    ..
                }
            ),
            "{err}"
        );
    }

    fn update_tx(
        owner: Pubkey,
        location: Option<u64>,
        elevation: Option<i32>,
        gain: Option<i32>,
    ) -> VersionedTransaction {
        let mut body = Vec::new();
        opt_u64(&mut body, location);
        opt_i32(&mut body, elevation);
        opt_i32(&mut body, gain);
        body.extend_from_slice(&[0u8; 32 * 3]);
        body.extend_from_slice(&0u32.to_le_bytes());
        let mut a = filler(3);
        a.push(owner);
        tx(KnownProgram::HeliumEntityManager, D_UPDATE_IOT, &body, &a)
    }

    #[test]
    fn the_requested_hotspot_update_is_accepted() {
        let w = Pubkey::new_unique();
        assert_hotspot_update(
            &[update_tx(w, Some(cell(LAT, LON)), None, None)],
            &w,
            Some(LAT),
            Some(LON),
            false,
            false,
        )
        .expect("the requested assertion");
    }

    #[test]
    fn an_update_at_another_location_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_hotspot_update(
            &[update_tx(w, Some(cell(51.5074, -0.1278)), None, None)],
            &w,
            Some(LAT),
            Some(LON),
            false,
            false,
        )
        .expect_err("a substituted location must be refused");
        assert!(
            matches!(
                err,
                VerifyError::ActionValue {
                    field: "location",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn an_update_setting_a_gain_that_was_not_requested_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_hotspot_update(
            &[update_tx(w, Some(cell(LAT, LON)), None, Some(30))],
            &w,
            Some(LAT),
            Some(LON),
            false,
            false,
        )
        .expect_err("an unrequested gain must be refused");
        assert!(
            matches!(err, VerifyError::ActionUnrequested { field: "gain", .. }),
            "{err}"
        );
    }

    #[test]
    fn an_update_setting_a_gain_that_was_requested_is_accepted() {
        // The value is the service's to convert; only its presence is checked.
        let w = Pubkey::new_unique();
        assert_hotspot_update(
            &[update_tx(w, Some(cell(LAT, LON)), None, Some(30))],
            &w,
            Some(LAT),
            Some(LON),
            true,
            false,
        )
        .expect("a requested gain");
    }

    #[test]
    fn an_update_for_another_owner_is_refused() {
        let w = Pubkey::new_unique();
        let err = assert_hotspot_update(
            &[update_tx(
                Pubkey::new_unique(),
                Some(cell(LAT, LON)),
                None,
                None,
            )],
            &w,
            Some(LAT),
            Some(LON),
            false,
            false,
        )
        .expect_err("another owner must be refused");
        assert!(
            matches!(err, VerifyError::ActionAccount { role: "owner", .. }),
            "{err}"
        );
    }

    #[test]
    fn a_second_update_is_refused() {
        let w = Pubkey::new_unique();
        let t = update_tx(w, Some(cell(LAT, LON)), None, None);
        let err = assert_hotspot_update(&[t.clone(), t], &w, Some(LAT), Some(LON), false, false)
            .expect_err("a batch must be refused");
        assert!(
            matches!(err, VerifyError::ActionCount { found: 2, .. }),
            "{err}"
        );
    }
}
