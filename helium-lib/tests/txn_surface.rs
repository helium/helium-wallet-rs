//! The transaction-construction surface external consumers compile against.
//!
//! Each path below is imported by a repo outside this workspace, named in the
//! comment above it. Removing one is a breaking change for that consumer, and
//! this file fails to compile rather than letting the break be discovered by a
//! `cargo update` over there.
#![allow(unused_imports)]

// The add-gateway token helpers carry no Solana construction, so a consumer
// that mints tokens and takes its transactions from the blockchain-api reaches
// them without the `txn` feature. Named here so that stays true.
use helium_lib::hotspot::dataonly::{issue_token, issue_token_to_add_tx, IssueHotspot, IssueToken};

#[test]
fn add_gateway_helpers_resolve_without_the_txn_feature() {
    let keypair = helium_crypto::Keypair::generate(Default::default(), &mut rand::rngs::OsRng);
    let issued = issue_token(&keypair).expect("mint an add-gateway token");

    // helium-solana-api serves this struct as a response body, so the field
    // names are a cross-repo wire contract, not an internal detail.
    let json = serde_json::to_value(&issued).expect("serialize the issued token");
    let token = json["token"].as_str().expect("a `token` string field");
    assert!(json["hotspot"]["key"].is_string(), "a `hotspot.key` field");
    assert!(
        json["hotspot"]["name"].is_string(),
        "a `hotspot.name` field"
    );

    let decoded = issue_token_to_add_tx(token).expect("decode it back");
    assert_eq!(decoded.gateway, keypair.public_key().to_vec());
}

#[cfg(feature = "txn")]
mod txn_feature {
    #![allow(unused_imports)]
    use super::*;

    // alembic bundles a hotspot location reset and an asset transfer into one
    // atomic transaction, sharing a single merkle proof to stay inside the
    // 1232-byte packet limit. That needs the instruction layer: the blockchain-api
    // builds one action per request, so two calls would give two transactions, two
    // proofs, and a window where the hotspot is reset but still owned.
    use helium_lib::{
        asset::transfer_instruction,
        dao::SubDao,
        dc::mint,
        hotspot::{direct_update, direct_update_instruction, transfer, HotspotInfoUpdate},
        message::{mk_budgeted_message, mk_raw_message},
        priority_fee::{compute_budget_instruction, compute_price_instruction},
        queue::claim_wallet,
        token::{transfer as token_transfer, transfer_instructions},
        transaction::mk_transaction,
        TransactionOpts,
    };

    // helium-solana-api serves a transaction shape its own consumers depend on, so
    // it builds locally rather than proxying the blockchain-api.
    use helium_lib::{
        hotspot::{
            dataonly::{
                issue_token, issue_token_to_add_tx, issue_transaction, onboard_transaction,
                IssueToken,
            },
            direct_update_transaction, transfer_transaction,
        },
        priority_fee::MIN_PRIORITY_FEE,
    };

    #[test]
    fn default_opts_target_mainnet() {
        let opts = TransactionOpts::default();
        assert!(opts.min_priority_fee <= opts.max_priority_fee);
        // Every consumer builds from `default()`, so this list IS the cluster the
        // library assumes. Asserting only that it is non-empty lets a devnet LUT
        // through, which would compile each consumer's mainnet transaction against
        // the wrong lookup table.
        assert_eq!(
            opts.lut_addresses,
            vec![helium_lib::message::COMMON_LUT],
            "the default must be the mainnet common LUT"
        );
    }

    /// Both consumers build a `HotspotInfoUpdate` through these methods rather than
    /// constructing the enum, so the type resolving is not enough.
    #[test]
    fn hotspot_info_update_builders_resolve() {
        let update = HotspotInfoUpdate::for_subdao(SubDao::Mobile)
            .set_geo(Some(37.7), Some(-122.4))
            .expect("a valid lat/lon");
        assert!(update.location().is_some());
    }
}
