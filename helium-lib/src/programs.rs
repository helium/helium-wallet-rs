#![allow(clippy::too_many_arguments)]
use anchor_lang::prelude::*;

pub const TOKEN_METADATA_PROGRAM_ID: Pubkey =
    pubkey!("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");

pub const SPL_NOOP_PROGRAM_ID: Pubkey = pubkey!("noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV");

declare_program!(helium_sub_daos);
declare_program!(lazy_distributor);
declare_program!(circuit_breaker);
declare_program!(helium_entity_manager);
declare_program!(data_credits);
declare_program!(hexboosting);
declare_program!(rewards_oracle);
declare_program!(spl_account_compression);
declare_program!(bubblegum);
declare_program!(iot_routing_manager);
