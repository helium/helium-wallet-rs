use crate::{client::GetAnchorAccount, error::Error, programs::TOKEN_METADATA_PROGRAM_ID};
use anchor_lang::{prelude::*, InstructionData};
use futures::{future::BoxFuture, Stream, StreamExt};
use itertools::Itertools;
use solana_sdk::{hash::hash, instruction::Instruction};
use spl_associated_token_account::get_associated_token_address;
use std::{ops::Range, result::Result, sync::Arc};

use helium_anchor_gen::iot_routing_manager::{self, typedefs::*};

declare_id!("irtjLnjCMmyowq2m3KWqpuFB3M9gdNA9A4t4d6VWmzB");

pub fn routing_manager_key(sub_dao: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"routing_manager", sub_dao.as_ref()], &ID).0
}

pub fn organization_key(routing_manager: &Pubkey, oui: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"organization",
            routing_manager.as_ref(),
            &oui.to_le_bytes(),
        ],
        &ID,
    )
    .0
}

pub fn devaddr_constraint_key(organization: &Pubkey, start_addr: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"devaddr_constraint",
            organization.as_ref(),
            &start_addr.to_le_bytes(),
        ],
        &ID,
    )
    .0
}

pub fn net_id_key(routing_manager: &Pubkey, net_id: u32) -> Pubkey {
    Pubkey::find_program_address(
        &[b"net_id", routing_manager.as_ref(), &net_id.to_le_bytes()],
        &ID,
    )
    .0
}

pub fn organization_delegate_key(organization: &Pubkey, delegate: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"organization_delegate",
            organization.as_ref(),
            delegate.as_ref(),
        ],
        &ID,
    )
    .0
}

pub fn routing_manager_collection_key(routing_manager: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"collection", routing_manager.as_ref()], &ID).0
}

pub fn routing_manager_collection_metadata_key(collection: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            collection.as_ref(),
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    )
    .0
}

pub fn routing_manager_collection_master_edition_key(collection: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            collection.as_ref(),
            b"edition",
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    )
    .0
}

pub mod organization {
    use super::*;
    use crate::{
        client::GetAnchorAccount,
        error::Error,
        programs::{BUBBLEGUM_PROGRAM_ID, SPL_ACCOUNT_COMPRESSION_PROGRAM_ID, SPL_NOOP_PROGRAM_ID},
    };
    use iot_routing_manager::IotRoutingManagerV0;

    pub async fn create<C: GetAnchorAccount>(
        client: &C,
        payer: Pubkey,
        authority: Option<Pubkey>,
        routing_manager_key: Pubkey,
    ) -> Result<(Pubkey, Instruction), Error> {
        let routing_manager: IotRoutingManagerV0 = client
            .anchor_account(&routing_manager_key)
            .await?
            .ok_or_else(|| Error::account_not_found())?;

        let oui = routing_manager.next_oui_id;
        let organization_key = organization_key(&routing_manager_key, oui);

        Ok((
            organization_key,
            Instruction {
                program_id: ID,
                accounts: iot_routing_manager::accounts::InitializeOrganizationV0 {
                    payer,
                    organization: organization_key,

                    bubblegum_program: BUBBLEGUM_PROGRAM_ID,
                    token_metadata_program: TOKEN_METADATA_PROGRAM_ID,
                    log_wrapper: SPL_NOOP_PROGRAM_ID,
                    compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                    token_program: anchor_spl::token::ID,
                    system_program: solana_sdk::system_program::ID,
                }
                .to_account_metas(None),
                data: iot_routing_manager::instruction::InitializeOrganizationV0 {}.data(),
            },
        ))
    }
}

pub mod orgainization_delegate {}

pub mod net_id {}

pub mod devaddr_constraint {}
