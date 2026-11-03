//! The surface external consumers compile against.
//!
//! Each path below is imported by a repo outside this workspace, named in the
//! comment above it. Removing one is a breaking change for that consumer, and
//! this file fails to compile rather than letting the break be discovered by a
//! `cargo update` over there.
#![allow(unused_imports)]

// alembic bundles a hotspot location reset and an asset transfer into one
// atomic transaction, sharing a single merkle proof to stay inside the
// 1232-byte packet limit. That needs the instruction layer: the blockchain-api
// builds one action per request, so two calls would give two transactions, two
// proofs, and a window where the hotspot is reset but still owned.
use helium_lib::{
    asset::{for_kta_with_proof, transfer_instruction},
    dao::SubDao,
    dc::mint,
    hotspot::{direct_update, direct_update_instruction, transfer, HotspotInfoUpdate},
    kta,
    message::{mk_budgeted_message, mk_raw_message},
    priority_fee::{compute_budget_instruction, compute_price_instruction, MAX_PRIORITY_FEE},
    programs::KnownProgram,
    queue::claim_wallet,
    token::{transfer as token_transfer, transfer_instructions},
    transaction::mk_transaction,
    TransactionOpts,
};

// helium-solana-api takes every transaction it serves from the blockchain-api
// and holds it to what was asked for, so it reaches the reading and checking
// surface rather than the instruction layer.
use helium_lib::{
    blockchain_api::{BlockchainApiError, Client as BlockchainApiClient},
    hotspot::{
        dataonly::{issue_token, issue_token_to_add_tx, IssueHotspot, IssueToken},
        get as hotspot_get, name as hotspot_name, search as hotspot_search, HotspotMode,
        HOTSPOT_CREATOR,
    },
    verify::{
        assert_asset_transfer, assert_hotspot_issue, assert_hotspot_onboard, assert_hotspot_update,
        sole_signable, VerifyError,
    },
};

/// helium-solana-api serves this struct as a response body, so its field names
/// are a cross-repo wire contract rather than an internal detail.
#[test]
fn add_gateway_token_round_trips_through_its_serialized_shape() {
    let keypair = helium_crypto::Keypair::generate(Default::default(), &mut rand::rngs::OsRng);
    let issued = issue_token(&keypair).expect("mint an add-gateway token");

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
