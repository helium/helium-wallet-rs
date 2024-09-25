use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    asset,
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    dao::{Dao, SubDao},
    data_credits,
    entity_key::AsEntityKey,
    error::{DecodeError, EncodeError, Error},
    helium_entity_manager, helium_sub_daos,
    hotspot::{HotspotInfoUpdate, ECC_VERIFIER},
    keypair::{Keypair, Pubkey},
    kta, priority_fee,
    programs::{
        SPL_ACCOUNT_COMPRESSION_PROGRAM_ID, SPL_NOOP_PROGRAM_ID, TOKEN_METADATA_PROGRAM_ID,
    },
    solana_sdk::{instruction::Instruction, signature::Signer, transaction::Transaction},
    token::Token,
};
use helium_proto::{BlockchainTxnAddGatewayV1, Message};
use serde::{Deserialize, Serialize};

mod iot {
    use super::*;

    pub async fn onboard<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
        client: &C,
        hotspot_key: &helium_crypto::PublicKey,
        assertion: HotspotInfoUpdate,
        keypair: &Keypair,
    ) -> Result<Transaction, Error> {
        fn mk_accounts(
            config_account: helium_entity_manager::DataOnlyConfigV0,
            owner: Pubkey,
            hotspot_key: &helium_crypto::PublicKey,
        ) -> impl ToAccountMetas {
            use helium_entity_manager::accounts::OnboardDataOnlyIotHotspotV0;
            let dao = Dao::Hnt;
            let entity_key = hotspot_key.as_entity_key();
            let data_only_config_key = dao.dataonly_config_key();
            OnboardDataOnlyIotHotspotV0 {
                payer: owner,
                dc_fee_payer: owner,
                iot_info: SubDao::Iot.info_key(&entity_key),
                hotspot_owner: owner,
                merkle_tree: config_account.merkle_tree,
                dc_burner: Token::Dc.associated_token_adress(&owner),
                rewardable_entity_config: SubDao::Iot.rewardable_entity_config_key(),
                data_only_config: data_only_config_key,
                dao: dao.key(),
                key_to_asset: dao.entity_key_to_kta_key(&entity_key),
                sub_dao: SubDao::Iot.key(),
                dc_mint: *Token::Dc.mint(),
                dc: SubDao::dc_key(),
                compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                data_credits_program: data_credits::id(),
                helium_sub_daos_program: helium_sub_daos::id(),
                token_program: anchor_spl::token::ID,
                associated_token_program: spl_associated_token_account::id(),
                system_program: solana_sdk::system_program::id(),
            }
        }

        let config_account = client
            .anchor_account::<helium_entity_manager::DataOnlyConfigV0>(
                &Dao::Hnt.dataonly_config_key(),
            )
            .await?;
        let kta = kta::for_entity_key(hotspot_key).await?;
        let (asset, asset_proof) = asset::for_kta_with_proof(client, &kta).await?;
        let mut onboard_accounts =
            mk_accounts(config_account, keypair.pubkey(), hotspot_key).to_account_metas(None);
        onboard_accounts.extend_from_slice(&asset_proof.proof(Some(3))?);

        let onboard_ix = solana_sdk::instruction::Instruction {
            program_id: helium_entity_manager::id(),
            accounts: onboard_accounts,
            data: helium_entity_manager::instruction::OnboardDataOnlyIotHotspotV0 {
                _args: helium_entity_manager::OnboardDataOnlyIotHotspotArgsV0 {
                    data_hash: asset.compression.data_hash,
                    creator_hash: asset.compression.creator_hash,
                    index: asset.compression.leaf_id()?,
                    root: asset_proof.root.to_bytes(),
                    elevation: *assertion.elevation(),
                    gain: assertion.gain_i32(),
                    location: assertion.location_u64(),
                },
            }
            .data(),
        };

        let ixs = &[
            priority_fee::compute_budget_instruction(300_000),
            priority_fee::compute_price_instruction_for_accounts(client, &onboard_ix.accounts)
                .await?,
            onboard_ix,
        ];

        let blockhash = AsRef::<SolanaRpcClient>::as_ref(client)
            .get_latest_blockhash()
            .await?;
        let tx =
            Transaction::new_signed_with_payer(ixs, Some(&keypair.pubkey()), &[keypair], blockhash);
        Ok(tx)
    }
}

mod mobile {
    use super::*;

    pub async fn onboard<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
        client: &C,
        hotspot_key: &helium_crypto::PublicKey,
        assertion: HotspotInfoUpdate,
        keypair: &Keypair,
    ) -> Result<Transaction, Error> {
        fn mk_accounts(
            config_account: helium_entity_manager::DataOnlyConfigV0,
            owner: Pubkey,
            hotspot_key: &helium_crypto::PublicKey,
        ) -> impl ToAccountMetas {
            use helium_entity_manager::accounts::OnboardDataOnlyMobileHotspotV0;
            let dao = Dao::Hnt;
            let entity_key = hotspot_key.as_entity_key();
            let data_only_config_key = dao.dataonly_config_key();
            OnboardDataOnlyMobileHotspotV0 {
                payer: owner,
                dc_fee_payer: owner,
                mobile_info: SubDao::Mobile.info_key(&entity_key),
                hotspot_owner: owner,
                merkle_tree: config_account.merkle_tree,
                dc_burner: Token::Dc.associated_token_adress(&owner),
                rewardable_entity_config: SubDao::Mobile.rewardable_entity_config_key(),
                data_only_config: data_only_config_key,
                dao: dao.key(),
                key_to_asset: dao.entity_key_to_kta_key(&entity_key),
                sub_dao: SubDao::Mobile.key(),
                dc_mint: *Token::Dc.mint(),
                dc: SubDao::dc_key(),
                dnt_mint: *Token::Mobile.mint(),
                dnt_price: *Token::Mobile.price_key().unwrap(), // safe to unwrap
                dnt_burner: Token::Mobile.associated_token_adress(&owner),
                compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
                data_credits_program: data_credits::id(),
                helium_sub_daos_program: helium_sub_daos::id(),
                token_program: anchor_spl::token::ID,
                associated_token_program: spl_associated_token_account::id(),
                system_program: solana_sdk::system_program::id(),
            }
        }

        let config_account = client
            .anchor_account::<helium_entity_manager::DataOnlyConfigV0>(
                &Dao::Hnt.dataonly_config_key(),
            )
            .await?;
        let kta = kta::for_entity_key(hotspot_key).await?;
        let (asset, asset_proof) = asset::for_kta_with_proof(client, &kta).await?;
        let mut onboard_accounts =
            mk_accounts(config_account, keypair.pubkey(), hotspot_key).to_account_metas(None);
        onboard_accounts.extend_from_slice(&asset_proof.proof(Some(3))?);

        let onboard_ix = solana_sdk::instruction::Instruction {
            program_id: helium_entity_manager::id(),
            accounts: onboard_accounts,
            data: helium_entity_manager::instruction::OnboardDataOnlyMobileHotspotV0 {
                _args: helium_entity_manager::OnboardDataOnlyMobileHotspotArgsV0 {
                    data_hash: asset.compression.data_hash,
                    creator_hash: asset.compression.creator_hash,
                    index: asset.compression.leaf_id()?,
                    root: asset_proof.root.to_bytes(),
                    location: assertion.location_u64(),
                },
            }
            .data(),
        };

        let ixs = &[
            priority_fee::compute_budget_instruction(300_000),
            priority_fee::compute_price_instruction_for_accounts(client, &onboard_ix.accounts)
                .await?,
            onboard_ix,
        ];

        let blockhash = AsRef::<SolanaRpcClient>::as_ref(client)
            .get_latest_blockhash()
            .await?;
        let tx =
            Transaction::new_signed_with_payer(ixs, Some(&keypair.pubkey()), &[keypair], blockhash);
        Ok(tx)
    }
}

pub async fn onboard<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    subdao: SubDao,
    hotspot_key: &helium_crypto::PublicKey,
    assertion: HotspotInfoUpdate,
    keypair: &Keypair,
) -> Result<Transaction, Error> {
    match subdao {
        SubDao::Iot => iot::onboard(client, hotspot_key, assertion, keypair).await,
        SubDao::Mobile => mobile::onboard(client, hotspot_key, assertion, keypair).await,
    }
}

pub async fn issue<C: AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    verifier: &str,
    add_tx: &mut BlockchainTxnAddGatewayV1,
    keypair: &Keypair,
) -> Result<Transaction, Error> {
    use helium_entity_manager::accounts::IssueDataOnlyEntityV0;
    fn mk_accounts(
        config_account: helium_entity_manager::DataOnlyConfigV0,
        owner: Pubkey,
        entity_key: &[u8],
    ) -> IssueDataOnlyEntityV0 {
        let dao = Dao::Hnt;
        let dataonly_config_key = dao.dataonly_config_key();
        IssueDataOnlyEntityV0 {
            payer: owner,
            ecc_verifier: ECC_VERIFIER,
            collection: config_account.collection,
            collection_metadata: dao.collection_metadata_key(&config_account.collection),
            collection_master_edition: dao
                .collection_master_edition_key(&config_account.collection),
            data_only_config: dataonly_config_key,
            entity_creator: dao.entity_creator_key(),
            dao: dao.key(),
            key_to_asset: dao.entity_key_to_kta_key(&entity_key),
            tree_authority: dao.merkle_tree_authority(&config_account.merkle_tree),
            recipient: owner,
            merkle_tree: config_account.merkle_tree,
            data_only_escrow: dao.dataonly_escrow_key(),
            bubblegum_signer: dao.bubblegum_signer(),
            token_metadata_program: TOKEN_METADATA_PROGRAM_ID,
            log_wrapper: SPL_NOOP_PROGRAM_ID,
            bubblegum_program: mpl_bubblegum::ID,
            compression_program: SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
            system_program: solana_sdk::system_program::id(),
        }
    }

    let config_account = client
        .anchor_account::<helium_entity_manager::DataOnlyConfigV0>(&Dao::Hnt.dataonly_config_key())
        .await?;
    let hotspot_key = helium_crypto::PublicKey::from_bytes(&add_tx.gateway)?;
    let entity_key = hotspot_key.as_entity_key();
    let accounts = mk_accounts(config_account, keypair.pubkey(), &entity_key);

    let issue_ix = Instruction {
        program_id: helium_entity_manager::id(),
        accounts: accounts.to_account_metas(None),
        data: helium_entity_manager::instruction::IssueDataOnlyEntityV0 {
            _args: helium_entity_manager::IssueDataOnlyEntityArgsV0 { entity_key },
        }
        .data(),
    };

    let ixs = &[
        priority_fee::compute_budget_instruction(300_000),
        priority_fee::compute_price_instruction_for_accounts(client, &accounts).await?,
        issue_ix,
    ];
    let mut tx = Transaction::new_with_payer(ixs, Some(&keypair.pubkey()));

    let blockhash = AsRef::<SolanaRpcClient>::as_ref(client)
        .get_latest_blockhash()
        .await?;
    tx.try_partial_sign(&[keypair], blockhash)?;

    let sig = add_tx.gateway_signature.clone();
    add_tx.gateway_signature = vec![];
    let msg = add_tx.encode_to_vec();

    let signed_tx = verify_helium_key(verifier, &msg, &sig, tx).await?;
    Ok(signed_tx)
}

async fn verify_helium_key(
    verifier: &str,
    msg: &[u8],
    signature: &[u8],
    tx: Transaction,
) -> Result<Transaction, Error> {
    #[derive(Deserialize, Serialize, Default)]
    struct VerifyRequest<'a> {
        // hex encoded solana transaction
        pub transaction: &'a str,
        // hex encoded signed message
        pub msg: &'a str,
        // hex encoded signature
        pub signature: &'a str,
    }
    #[derive(Deserialize, Serialize, Default)]
    struct VerifyResponse {
        // hex encoded solana transaction
        pub transaction: String,
    }
    let client = reqwest::Client::new();
    let serialized_tx = hex::encode(bincode::serialize(&tx).map_err(EncodeError::from)?);
    let response = client
        .post(format!("{}/verify", verifier))
        .json(&VerifyRequest {
            transaction: &serialized_tx,
            msg: &hex::encode(msg),
            signature: &hex::encode(signature),
        })
        .send()
        .await?
        .json::<VerifyResponse>()
        .await?;
    let signed_tx =
        bincode::deserialize(&hex::decode(response.transaction).map_err(DecodeError::from)?)
            .map_err(DecodeError::from)?;
    Ok(signed_tx)
}
