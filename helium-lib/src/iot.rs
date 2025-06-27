use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    bubblegum, data_credits, helium_entity_manager, iot_routing_manager,
    keypair::Pubkey,
    programs::TOKEN_METADATA_PROGRAM_ID,
};

use sha2::{Digest, Sha256};
use solana_sdk::instruction::Instruction;
use spl_associated_token_account::get_associated_token_address;
use std::result::Result;

pub fn routing_manager_key(sub_dao: &Pubkey) -> Pubkey {
    let (key, _) = Pubkey::find_program_address(
        &[b"routing_manager", sub_dao.as_ref()],
        &iot_routing_manager::ID,
    );
    key
}

pub fn organization_key(routing_manager: &Pubkey, oui: u64) -> Pubkey {
    let (key, _) = Pubkey::find_program_address(
        &[
            b"organization",
            routing_manager.as_ref(),
            &oui.to_le_bytes(),
        ],
        &iot_routing_manager::ID,
    );
    key
}

pub fn devaddr_constraint_key(organization: &Pubkey, start_addr: u64) -> Pubkey {
    let (key, _) = Pubkey::find_program_address(
        &[
            b"devaddr_constraint",
            organization.as_ref(),
            &start_addr.to_le_bytes(),
        ],
        &iot_routing_manager::ID,
    );
    key
}

pub fn net_id_key(routing_manager: &Pubkey, net_id: u32) -> Pubkey {
    let (key, _) = Pubkey::find_program_address(
        &[b"net_id", routing_manager.as_ref(), &net_id.to_le_bytes()],
        &iot_routing_manager::ID,
    );
    key
}

pub fn organization_delegate_key(organization: &Pubkey, delegate: &Pubkey) -> Pubkey {
    let (key, _) = Pubkey::find_program_address(
        &[
            b"organization_delegate",
            organization.as_ref(),
            delegate.as_ref(),
        ],
        &iot_routing_manager::ID,
    );
    key
}

pub fn organization_collection_key(routing_manager: &Pubkey) -> Pubkey {
    let (key, _) = Pubkey::find_program_address(
        &[b"collection", routing_manager.as_ref()],
        &iot_routing_manager::ID,
    );
    key
}

pub fn organization_collection_metadata_key(collection: &Pubkey) -> Pubkey {
    let (key, _) = Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            collection.as_ref(),
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    );
    key
}

pub fn organization_collection_master_edition_key(collection: &Pubkey) -> Pubkey {
    let (key, _) = Pubkey::find_program_address(
        &[
            b"metadata",
            TOKEN_METADATA_PROGRAM_ID.as_ref(),
            collection.as_ref(),
            b"edition",
        ],
        &TOKEN_METADATA_PROGRAM_ID,
    );
    key
}

pub fn organization_key_to_asset(dao: &Pubkey, oui: u64) -> Pubkey {
    let seed_str = format!("OUI_{}", oui);
    let hash = Sha256::digest(seed_str.as_bytes());
    let (key, _) = Pubkey::find_program_address(
        &[b"key_to_asset", dao.as_ref(), &hash],
        &helium_entity_manager::ID,
    );
    key
}

pub mod organization {
    use super::*;

    use crate::{
        asset,
        client::{GetAnchorAccount, SolanaRpcClient},
        dao::{Dao, SubDao},
        data_credits,
        error::Error,
        helium_entity_manager, iot_routing_manager,
        programs::SPL_NOOP_PROGRAM_ID,
        spl_account_compression,
        token::Token,
    };

    pub enum OrgIdentifier {
        Oui(u64),
        Pubkey(Pubkey),
    }

    pub async fn ensure_exists<C: AsRef<SolanaRpcClient>>(
        client: &C,
        identifier: OrgIdentifier,
    ) -> Result<(Pubkey, iot_routing_manager::accounts::OrganizationV0), Error> {
        let sub_dao = SubDao::Iot.key();
        let routing_manager_key = routing_manager_key(&sub_dao);
        let organization_key = match identifier {
            OrgIdentifier::Oui(oui) => organization_key(&routing_manager_key, oui),
            OrgIdentifier::Pubkey(pubkey) => pubkey,
        };

        match client
            .as_ref()
            .anchor_account::<iot_routing_manager::accounts::OrganizationV0>(&organization_key)
            .await
        {
            Ok(organization) => Ok((organization_key, organization)),
            Err(_) => Err(Error::account_not_found()),
        }
    }

    pub async fn create<C: AsRef<SolanaRpcClient>>(
        client: &C,
        payer: Pubkey,
        net_id_key: Pubkey,
        authority: Option<Pubkey>,
        recipient: Option<Pubkey>,
    ) -> Result<(Pubkey, Instruction), Error> {
        let payer_dc_ata_key = get_associated_token_address(&payer, Token::Dc.mint());
        let dao_key = Dao::Hnt.key();
        let sub_dao = SubDao::Iot.key();
        let program_approval_key = Dao::Hnt.program_approval_key(&iot_routing_manager::ID);

        client
            .as_ref()
            .get_account(&payer_dc_ata_key)
            .await
            .map_err(|_| Error::AccountAbsent("Payer DC token account.".to_string()))?;

        client
            .as_ref()
            .anchor_account::<helium_entity_manager::accounts::ProgramApprovalV0>(
                &program_approval_key,
            )
            .await?;

        let shared_merkle_key = asset::shared_merkle_key(3);
        let shared_merkle = client
            .as_ref()
            .anchor_account::<helium_entity_manager::accounts::SharedMerkleV0>(&shared_merkle_key)
            .await?;

        let routing_manager_key = routing_manager_key(&sub_dao);
        let routing_manager = client
            .as_ref()
            .anchor_account::<iot_routing_manager::accounts::IotRoutingManagerV0>(
                &routing_manager_key,
            )
            .await?;

        client
            .as_ref()
            .anchor_account::<iot_routing_manager::accounts::NetIdV0>(&net_id_key)
            .await?;

        let oui = routing_manager.next_oui_id;
        let organization_key = organization_key(&routing_manager_key, oui);
        let collection_key = organization_collection_key(&routing_manager_key);

        Ok((
            organization_key,
            Instruction {
                program_id: iot_routing_manager::ID,
                accounts: iot_routing_manager::client::accounts::InitializeOrganizationV0 {
                    payer,
                    program_approval: program_approval_key,
                    routing_manager: routing_manager_key,
                    net_id: net_id_key,
                    dc_mint: *Token::Dc.mint(),
                    data_credits: Dao::dc_key(),
                    payer_dc_account: payer_dc_ata_key,
                    authority: authority.unwrap_or(payer),
                    bubblegum_signer: asset::bubblegum_signer_key(),
                    shared_merkle: shared_merkle_key,
                    helium_entity_manager_program: helium_entity_manager::ID,
                    dao: dao_key,
                    sub_dao: routing_manager.sub_dao,
                    organization: organization_key,
                    collection: collection_key,
                    collection_metadata: organization_collection_metadata_key(&collection_key),
                    collection_master_edition: organization_collection_master_edition_key(
                        &collection_key,
                    ),
                    entity_creator: Dao::Hnt.entity_creator_key(),
                    key_to_asset: organization_key_to_asset(&dao_key, oui),
                    tree_authority: asset::merkle_tree_authority_key(&shared_merkle.merkle_tree),
                    recipient: recipient.unwrap_or(payer),
                    merkle_tree: shared_merkle.merkle_tree,
                    bubblegum_program: bubblegum::ID,
                    token_metadata_program: TOKEN_METADATA_PROGRAM_ID,
                    log_wrapper: SPL_NOOP_PROGRAM_ID,
                    compression_program: spl_account_compression::ID,
                    data_credits_program: data_credits::ID,
                    token_program: anchor_spl::token::ID,
                    associated_token_program: spl_associated_token_account::ID,
                    system_program: solana_sdk::system_program::ID,
                }
                .to_account_metas(None),
                data: iot_routing_manager::client::args::InitializeOrganizationV0 {}.data(),
            },
        ))
    }

    pub async fn approve<C: AsRef<SolanaRpcClient>>(
        _client: &C,
        authority: Pubkey,
        organization_key: Pubkey,
        net_id_key: Pubkey,
    ) -> Result<Instruction, Error> {
        Ok(Instruction {
            program_id: iot_routing_manager::ID,
            accounts: iot_routing_manager::client::accounts::ApproveOrganizationV0 {
                authority,
                organization: organization_key,
                net_id: net_id_key,
                system_program: solana_sdk::system_program::ID,
            }
            .to_account_metas(None),
            data: iot_routing_manager::client::args::ApproveOrganizationV0 {}.data(),
        })
    }

    pub async fn update<C: AsRef<SolanaRpcClient>>(
        _client: &C,
        authority: Pubkey,
        organization_key: Pubkey,
        args: iot_routing_manager::types::UpdateOrganizationArgsV0,
    ) -> Result<Instruction, Error> {
        Ok(Instruction {
            program_id: iot_routing_manager::ID,
            accounts: iot_routing_manager::client::accounts::UpdateOrganizationV0 {
                authority,
                organization: organization_key,
            }
            .to_account_metas(None),
            data: iot_routing_manager::client::args::UpdateOrganizationV0 { args }.data(),
        })
    }
}

pub mod organization_delegate {
    use super::*;

    use crate::{client::SolanaRpcClient, error::Error, iot_routing_manager};

    pub async fn create<C: AsRef<SolanaRpcClient>>(
        _client: &C,
        payer: Pubkey,
        delegate: Pubkey,
        organization_key: Pubkey,
        authority: Option<Pubkey>,
    ) -> Result<(Pubkey, Instruction), Error> {
        let organization_delegate_key = organization_delegate_key(&organization_key, &delegate);

        Ok((
            organization_delegate_key,
            Instruction {
                program_id: iot_routing_manager::ID,
                accounts: iot_routing_manager::client::accounts::InitializeOrganizationDelegateV0 {
                    payer,
                    authority: authority.unwrap_or(payer),
                    delegate,
                    organization: organization_key,
                    organization_delegate: organization_delegate_key,
                    system_program: solana_sdk::system_program::ID,
                }
                .to_account_metas(None),
                data: iot_routing_manager::client::args::InitializeOrganizationDelegateV0 {}.data(),
            },
        ))
    }

    pub async fn remove<C: AsRef<SolanaRpcClient>>(
        _client: &C,
        authority: Pubkey,
        delegate: Pubkey,
        organization_key: Pubkey,
    ) -> Result<Instruction, Error> {
        let organization_delegate_key = organization_delegate_key(&organization_key, &delegate);

        Ok(Instruction {
            program_id: iot_routing_manager::ID,
            accounts: iot_routing_manager::client::accounts::RemoveOrganizationDelegateV0 {
                authority,
                rent_refund: authority,
                organization: organization_key,
                organization_delegate: organization_delegate_key,
            }
            .to_account_metas(None),
            data: iot_routing_manager::client::args::RemoveOrganizationDelegateV0 {}.data(),
        })
    }
}

pub mod net_id {
    use super::*;

    use crate::{
        client::{GetAnchorAccount, SolanaRpcClient},
        dao::SubDao,
        error::Error,
        iot_routing_manager,
    };

    pub enum NetIdIdentifier {
        Id(u32),
        Pubkey(Pubkey),
    }

    pub async fn ensure_exists<C: AsRef<SolanaRpcClient>>(
        client: &C,
        identifier: NetIdIdentifier,
    ) -> Result<(Pubkey, iot_routing_manager::accounts::NetIdV0), Error> {
        let sub_dao = SubDao::Iot.key();
        let routing_manager_key = routing_manager_key(&sub_dao);
        let net_id_key = match identifier {
            NetIdIdentifier::Id(id) => net_id_key(&routing_manager_key, id),
            NetIdIdentifier::Pubkey(pubkey) => pubkey,
        };

        match client
            .as_ref()
            .anchor_account::<iot_routing_manager::accounts::NetIdV0>(&net_id_key)
            .await
        {
            Ok(net_id) => Ok((net_id_key, net_id)),
            Err(_) => Err(Error::account_not_found()),
        }
    }

    pub async fn create<C: AsRef<SolanaRpcClient>>(
        client: &C,
        payer: Pubkey,
        args: iot_routing_manager::types::InitializeNetIdArgsV0,
        authority: Option<Pubkey>,
    ) -> Result<(Pubkey, Instruction), Error> {
        let sub_dao = SubDao::Iot.key();
        let routing_manager_key = routing_manager_key(&sub_dao);
        let routing_manager = client
            .as_ref()
            .anchor_account::<iot_routing_manager::accounts::IotRoutingManagerV0>(
                &routing_manager_key,
            )
            .await?;

        let net_id_key = net_id_key(&routing_manager_key, args.net_id);
        let net_id_exists = client
            .as_ref()
            .anchor_account::<iot_routing_manager::accounts::NetIdV0>(&net_id_key)
            .await
            .is_ok();

        if net_id_exists {
            return Err(Error::account_exists());
        }

        Ok((
            net_id_key,
            Instruction {
                program_id: iot_routing_manager::ID,
                accounts: iot_routing_manager::client::accounts::InitializeNetIdV0 {
                    payer,
                    routing_manager: routing_manager_key,
                    net_id_authority: routing_manager.net_id_authority,
                    authority: authority.unwrap_or(payer),
                    net_id: net_id_key,
                    system_program: solana_sdk::system_program::ID,
                }
                .to_account_metas(None),
                data: iot_routing_manager::client::args::InitializeNetIdV0 { args }.data(),
            },
        ))
    }
}

pub mod devaddr_constraint {
    use super::*;

    use crate::{
        client::{GetAnchorAccount, SolanaRpcClient},
        dao::{Dao, SubDao},
        error::Error,
        iot_routing_manager,
        token::Token,
    };

    pub async fn create<C: AsRef<SolanaRpcClient>>(
        client: &C,
        payer: Pubkey,
        args: iot_routing_manager::types::InitializeDevaddrConstraintArgsV0,
        organization_key: Pubkey,
        net_id_key: Pubkey,
        authority: Option<Pubkey>,
    ) -> Result<(Pubkey, Instruction), Error> {
        let payer_dc_ata_key = get_associated_token_address(&payer, Token::Dc.mint());
        let sub_dao = SubDao::Iot.key();
        let routing_manager_key = routing_manager_key(&sub_dao);

        client
            .as_ref()
            .get_account(&payer_dc_ata_key)
            .await
            .map_err(|_| Error::AccountAbsent("Payer DC token account.".to_string()))?;

        let net_id = client
            .as_ref()
            .anchor_account::<iot_routing_manager::accounts::NetIdV0>(&net_id_key)
            .await?;

        let devaddr_constarint_key =
            devaddr_constraint_key(&organization_key, net_id.current_addr_offset);

        Ok((
            devaddr_constarint_key,
            Instruction {
                program_id: iot_routing_manager::ID,
                accounts: iot_routing_manager::client::accounts::InitializeDevaddrConstraintV0 {
                    payer,
                    authority: authority.unwrap_or(payer),
                    net_id: net_id_key,
                    routing_manager: routing_manager_key,
                    organization: organization_key,
                    dc_mint: *Token::Dc.mint(),
                    data_credits: Dao::dc_key(),
                    payer_dc_account: payer_dc_ata_key,
                    devaddr_constraint: devaddr_constarint_key,
                    data_credits_program: data_credits::ID,
                    token_program: anchor_spl::token::ID,
                    associated_token_program: spl_associated_token_account::ID,
                    system_program: solana_sdk::system_program::ID,
                }
                .to_account_metas(None),
                data: iot_routing_manager::client::args::InitializeDevaddrConstraintV0 { args }
                    .data(),
            },
        ))
    }

    pub async fn remove<C: AsRef<SolanaRpcClient>>(
        client: &C,
        authority: Pubkey,
        devaddr_constraint_key: Pubkey,
    ) -> Result<Instruction, Error> {
        let devaddr_constraint = client
            .as_ref()
            .anchor_account::<iot_routing_manager::accounts::DevaddrConstraintV0>(
                &devaddr_constraint_key,
            )
            .await?;

        Ok(Instruction {
            program_id: iot_routing_manager::ID,
            accounts: iot_routing_manager::client::accounts::RemoveDevaddrConstraintV0 {
                authority,
                rent_refund: authority,
                net_id: devaddr_constraint.net_id,
                devaddr_constraint: devaddr_constraint_key,
            }
            .to_account_metas(None),
            data: iot_routing_manager::client::args::RemoveDevaddrConstraintV0 {}.data(),
        })
    }
}
