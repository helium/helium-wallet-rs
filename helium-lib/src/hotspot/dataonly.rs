use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    asset, b64,
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    dao::{Dao, SubDao},
    data_credits,
    entity_key::AsEntityKey,
    error::{DecodeError, EncodeError, Error},
    helium_entity_manager, helium_sub_daos,
    hotspot::{self, HotspotInfoUpdate, ECC_VERIFIER},
    keypair::{Keypair, Pubkey},
    kta, mk_transaction_with_blockhash, priority_fee,
    programs::{
        SPL_ACCOUNT_COMPRESSION_PROGRAM_ID, SPL_NOOP_PROGRAM_ID, TOKEN_METADATA_PROGRAM_ID,
    },
    solana_sdk::{instruction::Instruction, signature::Signer},
    token::Token,
    TransactionOpts, TransactionWithBlockhash,
};
use helium_crypto::{PublicKey, Sign};
use helium_proto::{BlockchainTxn, BlockchainTxnAddGatewayV1, Message, Txn};
use serde::{Deserialize, Serialize};

mod iot {

    use super::*;

    pub async fn onboard_transaction<
        C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount,
    >(
        client: &C,
        hotspot_key: &helium_crypto::PublicKey,
        assertion: HotspotInfoUpdate,
        owner: &Pubkey,
        opts: &TransactionOpts,
    ) -> Result<TransactionWithBlockhash, Error> {
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
                dc: Dao::dc_key(),
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
            mk_accounts(config_account, *owner, hotspot_key).to_account_metas(None);
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
            priority_fee::compute_price_instruction_for_accounts(
                client,
                &onboard_ix.accounts,
                opts.min_priority_fee,
            )
            .await?,
            onboard_ix,
        ];

        mk_transaction_with_blockhash(client, ixs, owner).await
    }
}

mod mobile {

    use super::*;

    pub async fn onboard_transaction<
        C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount,
    >(
        client: &C,
        hotspot_key: &helium_crypto::PublicKey,
        assertion: HotspotInfoUpdate,
        owner: &Pubkey,
        opts: &TransactionOpts,
    ) -> Result<TransactionWithBlockhash, Error> {
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
                dc: Dao::dc_key(),
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
            mk_accounts(config_account, *owner, hotspot_key).to_account_metas(None);
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
            priority_fee::compute_price_instruction_for_accounts(
                client,
                &onboard_ix.accounts,
                opts.min_priority_fee,
            )
            .await?,
            onboard_ix,
        ];

        mk_transaction_with_blockhash(client, ixs, owner).await
    }
}

pub async fn onboard_transaction<
    C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount,
>(
    client: &C,
    subdao: SubDao,
    hotspot_key: &helium_crypto::PublicKey,
    assertion: HotspotInfoUpdate,
    owner: &Pubkey,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    match subdao {
        SubDao::Iot => iot::onboard_transaction(client, hotspot_key, assertion, owner, opts).await,
        SubDao::Mobile => {
            mobile::onboard_transaction(client, hotspot_key, assertion, owner, opts).await
        }
    }
}

pub async fn onboard<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    subdao: SubDao,
    hotspot_key: &helium_crypto::PublicKey,
    assertion: HotspotInfoUpdate,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let mut txn = onboard_transaction(
        client,
        subdao,
        hotspot_key,
        assertion,
        &keypair.pubkey(),
        opts,
    )
    .await?;
    txn.try_sign(&[keypair])?;
    Ok(txn)
}

pub async fn issue_transaction<C: AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    verifier: &str,
    add_tx: &mut BlockchainTxnAddGatewayV1,
    owner: Pubkey,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    fn mk_accounts(
        config_account: helium_entity_manager::DataOnlyConfigV0,
        owner: Pubkey,
        entity_key: &[u8],
    ) -> impl ToAccountMetas {
        use helium_entity_manager::accounts::IssueDataOnlyEntityV0;
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
    let accounts = mk_accounts(config_account, owner, &entity_key);

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
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &accounts,
            opts.min_priority_fee,
        )
        .await?,
        issue_ix,
    ];

    let txn = mk_transaction_with_blockhash(client, ixs, &owner).await?;

    let sig = add_tx.gateway_signature.clone();
    add_tx.gateway_signature = vec![];
    let msg = add_tx.encode_to_vec();

    let signed_txn = verify_helium_key(verifier, &msg, &sig, txn).await?;
    Ok(signed_txn)
}

#[derive(Debug, serde::Serialize)]
pub struct IssueHotspot {
    key: PublicKey,
    name: String,
}

impl From<&helium_crypto::Keypair> for IssueHotspot {
    fn from(value: &helium_crypto::Keypair) -> Self {
        Self {
            key: value.public_key().clone(),
            name: hotspot::name(value.public_key()),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct IssueToken {
    hotspot: IssueHotspot,
    token: String,
}

pub fn issue_token(gw_keypair: &helium_crypto::Keypair) -> Result<IssueToken, Error> {
    let mut txn = BlockchainTxnAddGatewayV1 {
        gateway: gw_keypair.public_key().to_vec(),
        gateway_signature: vec![],
        owner: vec![],
        owner_signature: vec![],
        payer: vec![],
        payer_signature: vec![],
        fee: 0,
        staking_fee: 0,
    };
    txn.gateway_signature = gw_keypair.sign(&txn.encode_to_vec())?;

    let envelope = BlockchainTxn {
        txn: Some(Txn::AddGateway(txn)),
    };
    Ok(IssueToken {
        hotspot: IssueHotspot::from(gw_keypair),
        token: b64::encode_message(&envelope)?,
    })
}

pub fn issue_token_to_add_tx(token: &str) -> Result<BlockchainTxnAddGatewayV1, Error> {
    let envelope: BlockchainTxn = b64::decode_message(token)?;
    match envelope.txn {
        Some(Txn::AddGateway(txn)) => Ok(txn),
        _ => Err(DecodeError::other("unsupported transaction").into()),
    }
}

pub async fn issue<C: AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    verifier: &str,
    add_tx: &mut BlockchainTxnAddGatewayV1,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let mut txn = issue_transaction(client, verifier, add_tx, keypair.pubkey(), opts).await?;
    txn.try_partial_sign(&[keypair])?;
    Ok(txn)
}

async fn verify_helium_key(
    verifier: &str,
    msg: &[u8],
    signature: &[u8],
    tx: TransactionWithBlockhash,
) -> Result<TransactionWithBlockhash, Error> {
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
    let serialized_tx = hex::encode(bincode::serialize(&tx.inner).map_err(EncodeError::from)?);
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
    Ok(tx.with_signed_transaction(signed_tx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn roundtrip_issue_token() {
        let gw_keypair = helium_crypto::Keypair::generate(Default::default(), &mut OsRng);
        let issue_token = issue_token(&gw_keypair).expect("issue token");
        let gw_pubkey =
            helium_crypto::PublicKey::try_from(issue_token.hotspot.key).expect("hotspot key");
        let decoded = issue_token_to_add_tx(&issue_token.token).expect("decoded issue token");
        let decoded_gw_pubkey =
            helium_crypto::PublicKey::try_from(decoded.gateway).expect("decoded hotspot key");
        assert_eq!(gw_pubkey, decoded_gw_pubkey);
    }
}
