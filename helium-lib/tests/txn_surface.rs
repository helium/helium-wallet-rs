//! The transaction-construction surface external consumers compile against.
//!
//! Each path below is imported by a repo outside this workspace, named in the
//! comment above it. Removing one is a breaking change for that consumer, and
//! this file fails to compile rather than letting the break be discovered by a
//! `cargo update` over there.
#![cfg(feature = "txn")]
#![allow(unused_imports)]

// alembic bundles a hotspot location reset and an asset transfer into one
// atomic transaction, sharing a single merkle proof to stay inside the
// 1232-byte packet limit. That needs the instruction layer: the blockchain-api
// builds one action per request, so two calls would give two transactions, two
// proofs, and a window where the hotspot is reset but still owned.
use helium_lib::{
    asset::transfer_instruction,
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
        dataonly::{issue_token, issue_token_to_add_tx, issue_transaction, onboard_transaction},
        direct_update_transaction, transfer_transaction,
    },
    priority_fee::MIN_PRIORITY_FEE,
};

#[test]
fn default_opts_carry_a_usable_fee_range() {
    let opts = TransactionOpts::default();
    assert!(opts.min_priority_fee <= opts.max_priority_fee);
    assert!(!opts.lut_addresses.is_empty(), "a common LUT is expected");
}
