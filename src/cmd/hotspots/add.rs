use crate::{
    cmd::*,
    dao::{Dao, SubDao},
    result::Result,
    token::Token,
    traits::txn_envelope::TxnEnvelope,
};
use anchor_client::solana_client::rpc_config::RpcSendTransactionConfig;
use anchor_client::{solana_sdk::signer::Signer, Program};
use anyhow::anyhow;
use bs58;
use data_credits::ID as DC_PID;
use helium_entity_manager::{
    accounts::{IssueDataOnlyEntityV0, OnboardDataOnlyIotHotspotV0},
    DataOnlyConfigV0, IssueDataOnlyEntityArgsV0, KeyToAssetV0, OnboardDataOnlyIotHotspotArgsV0,
    ECC_VERIFIER,
};
use helium_proto::Message;
use mpl_bubblegum::ID as BGUM_PID;
use serde::{Deserialize, Serialize};
use solana_program::instruction::AccountMeta;
use solana_program::system_program;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction, transaction::Transaction as SolanaTransaction,
};
use spl_associated_token_account::get_associated_token_address;
use std::{rc::Rc, str::FromStr};
use BlockchainTxnAddGatewayV1;

#[derive(Clone, Debug, clap::Args)]
/// Add a hotspot to the blockchain. The original transaction is created by the
/// hotspot miner and supplied here for owner signing. Use an onboarding key to
/// get the transaction signed by the DeWi staking server.
pub struct Cmd {
    /// Base64 encoded transaction. If no transaction is given stdin is
    /// read for the transaction. Note that the stdin feature only works if the
    /// wallet password is set in the HELIUM_WALLET_PASSWORD environment
    /// variable
    #[arg(long)]
    add_gateway_txn: Option<Transaction>,

    /// The location of the hotspot. If supplied, the location will be asserted
    #[arg(long)]
    location: Option<u64>,
    /// The elevation of the hotspot
    #[arg(long)]
    elevation: Option<i32>,
    /// The gain of the hotspot
    #[arg(long)]
    gain: Option<i32>,

    /// Optional url for the ecc signature verifier. Defaults to https://ecc-verifier.web.helium.io
    #[arg(long)]
    ecc_verifier_url: Option<String>,
}

#[derive(Deserialize, Serialize, Default)]
struct VerifyResponse {
    // hex encoded solana transaction
    pub transaction: String,
}

#[derive(Deserialize, Serialize, Default)]
struct VerifyRequest<'a> {
    // hex encoded solana transaction
    pub transaction: &'a str,
    // hex encoded signed message
    pub msg: &'a str,
    // hex encoded signature
    pub signature: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: serde_json::Value,
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse<T> {
    jsonrpc: String,
    result: Option<T>,
    error: Option<JsonRpcError>,
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let mut add_gateway_txn =
            BlockchainTxnAddGatewayV1::from_envelope(&read_txn(&self.add_gateway_txn)?)?;
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let anchor_client = client.settings.mk_anchor_client(keypair.clone())?;

        let program = anchor_client.program(helium_entity_manager::id());

        let default_url = &"https://ecc-verifier.web.helium.io".to_string();
        let ecc_verifier_url = match &self.ecc_verifier_url {
            Some(url) => url,
            None => default_url,
        };
        let entity_key = &add_gateway_txn.gateway;

        // check if entity has been issued by checking key_to_asset exists
        let key_to_asset = Dao::Hnt.key_to_asset(entity_key);

        let kta = program.rpc().get_account(&key_to_asset);
        // If the entity has not been issued, issue it. Otherwise, onboard it.
        if kta.is_err() {
            // construct the issue entity transaction
            let issue_entity_accounts = construct_issue_entity_accounts(&program, entity_key)
                .map_err(|e| anyhow!("Failed to create issue entity accounts: {e}"))?;
            let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(500000);

            let ix = program
                .request()
                .args(helium_entity_manager::instruction::IssueDataOnlyEntityV0 {
                    args: IssueDataOnlyEntityArgsV0 {
                        entity_key: entity_key.clone(),
                    },
                })
                .accounts(issue_entity_accounts)
                .instruction(compute_ix)
                .instructions()?;

            let mut tx = SolanaTransaction::new_with_payer(&ix, Some(&keypair.pubkey()));
            let blockhash = program.rpc().get_latest_blockhash()?;

            tx.try_partial_sign(&[Rc::as_ref(&keypair)], blockhash)
                .map_err(|e| anyhow!("Error while signing tx: {e}"))?;

            let serialized_tx = hex::encode(bincode::serialize(&tx)?);
            let sig = add_gateway_txn.gateway_signature.clone();
            add_gateway_txn.gateway_signature = vec![];

            let mut buf = vec![];
            add_gateway_txn.encode(&mut buf)?;

            // verify the base64 transaction with the ecc-sig-verifier
            let req_client = reqwest::blocking::Client::new();
            let response = req_client
                .post(format!("{}/verify", ecc_verifier_url))
                .json(&VerifyRequest {
                    transaction: &serialized_tx,
                    msg: &hex::encode(&buf),
                    signature: &hex::encode(sig),
                })
                .send()
                .map_err(|e| anyhow!("Error while sending request: {e}"))?
                .json::<VerifyResponse>()
                .map_err(|e| anyhow!("Error while parsing response: {e}"))?;
            let raw_signed_tx = hex::decode(response.transaction)?;
            let signed_tx: SolanaTransaction = bincode::deserialize(&raw_signed_tx)?;
            println!("Transaction signed: {:?}", signed_tx.is_signed());

            program
                .rpc()
                .send_and_confirm_transaction_with_spinner(&signed_tx)?;
        } else {
            println!("Entity already issued");
        }

        println!("Onboarding hotspot");

        let kta_acc = program
            .account::<KeyToAssetV0>(key_to_asset)
            .map_err(|e| anyhow!("Failed to get key_to_asset account: {e}"))?;
        let onboard_accounts = construct_onboard_iot_accounts(&program, entity_key)
            .map_err(|e| anyhow!("Failed to create onboard iot accounts: {e}"))?;

        let req_client = reqwest::blocking::Client::new();

        let get_asset_response = req_client
            .post(program.rpc().url())
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .header("Expires", "0")
            .json(&JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "getAsset".to_string(),
                params: json!({
                    "id": kta_acc.asset.to_string(),
                }),
                id: "rpd-op-123".to_string(),
            })
            .send()
            .map_err(|e| anyhow!("Failed to get asset: {e}"))?
            .json::<JsonRpcResponse<serde_json::Value>>()
            .map_err(|e| anyhow!("Failed to parse asset response: {e}"))?;
        let result = get_asset_response.result.unwrap();
        let data_hash: [u8; 32] = bs58::decode(
            result.as_object().unwrap()["compression"]["data_hash"]
                .as_str()
                .ok_or(anyhow!("Failed to get data_hash"))?,
        )
        .into_vec()
        .map_err(|e| anyhow!("Failed to decode data_hash: {e}"))?
        .as_slice()
        .try_into()
        .map_err(|e| anyhow!("Failed to convert data_hash: {e}"))?;
        let creator_hash: [u8; 32] = bs58::decode(
            result.as_object().unwrap()["compression"]["creator_hash"]
                .as_str()
                .ok_or(anyhow!("Failed to get creator_hash"))?,
        )
        .into_vec()
        .map_err(|e| anyhow!("Failed to decode creator_hash: {e}"))?
        .as_slice()
        .try_into()
        .map_err(|e| anyhow!("Failed to convert creator_hash: {e}"))?;
        let leaf_id = result.as_object().unwrap()["compression"]["leaf_id"]
            .as_u64()
            .ok_or(anyhow!("Failed to get leaf_id"))?;

        let req_client = reqwest::blocking::Client::new();
        let get_asset_proof_response = req_client
            .post(program.rpc().url())
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .header("Expires", "0")
            .json(&JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "getAssetProof".to_string(),
                params: json!({
                    "id": kta_acc.asset.to_string(),
                }),
                id: "rpd-op-123".to_string(),
            })
            .send()?
            .json::<JsonRpcResponse<serde_json::Value>>()?;

        let result = get_asset_proof_response.result.unwrap();
        let root: [u8; 32] =
            Pubkey::from_str(result.as_object().unwrap()["root"].as_str().unwrap())?.to_bytes();

        let proof: Vec<AccountMeta> = result.as_object().unwrap()["proof"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| AccountMeta {
                pubkey: Pubkey::from_str(p.as_str().unwrap()).unwrap(),
                is_signer: false,
                is_writable: false,
            })
            .collect();
        let iot_info = onboard_accounts.iot_info;
        let mut ixs = program
            .request()
            .args(
                helium_entity_manager::instruction::OnboardDataOnlyIotHotspotV0 {
                    args: OnboardDataOnlyIotHotspotArgsV0 {
                        data_hash,
                        creator_hash,
                        root,
                        index: leaf_id.try_into()?,
                        location: self.location,
                        elevation: self.elevation,
                        gain: self.gain,
                    },
                },
            )
            .accounts(onboard_accounts)
            .instructions()?;

        ixs[0].accounts.extend_from_slice(&proof.as_slice()[0..3]);
        let mut tx = SolanaTransaction::new_with_payer(&ixs, Some(&keypair.pubkey()));
        let blockhash = program.rpc().get_latest_blockhash()?;

        tx.try_sign(&[Rc::as_ref(&keypair)], blockhash)
            .map_err(|e| anyhow!("Error while signing tx: {e}"))?;
        program
            .rpc()
            .send_transaction_with_config(
                &tx,
                RpcSendTransactionConfig {
                    skip_preflight: true,
                    ..RpcSendTransactionConfig::default()
                },
            )
            .map_err(|e| anyhow!("Failed to send and confirm transaction: {e}"))?;

        println!(
            "Finished onboarding hotspot. \nHotspot asset ID: {}\nHotspot info pubkey: {}",
            kta_acc.asset.to_string(),
            iot_info.to_string(),
        );
        Ok(())
    }
}

fn construct_issue_entity_accounts(
    program: &Program,
    entity_key: &[u8],
) -> Result<IssueDataOnlyEntityV0> {
    use anchor_client::anchor_lang::Id;
    let token_metadata_pid = mpl_token_metadata::id();
    let noop_pid = spl_account_compression::Noop::id();
    let compression_pid = spl_account_compression::id();
    let dao = Dao::Hnt;

    let data_only_config = Dao::Hnt.data_only_config_key();

    let data_only_config_acc = program
        .account::<DataOnlyConfigV0>(data_only_config)
        .map_err(|e| anyhow!("Couldn't find data only config: {e}"))?;

    let (collection_metadata, _cm_bump) = Pubkey::find_program_address(
        &[
            "metadata".as_bytes(),
            token_metadata_pid.as_ref(),
            data_only_config_acc.collection.as_ref(),
        ],
        &token_metadata_pid,
    );

    let (collection_master_edition, _cme_bump) = Pubkey::find_program_address(
        &[
            "metadata".as_bytes(),
            token_metadata_pid.as_ref(),
            data_only_config_acc.collection.as_ref(),
            "edition".as_bytes(),
        ],
        &token_metadata_pid,
    );

    let entity_creator = dao.entity_creator_key();
    let key_to_asset = dao.key_to_asset(entity_key);

    let (tree_authority, _ta_bump) =
        Pubkey::find_program_address(&[data_only_config_acc.merkle_tree.as_ref()], &BGUM_PID);

    let (data_only_escrow, _doe_bump) = Pubkey::find_program_address(
        &["data_only_escrow".as_bytes(), data_only_config.as_ref()],
        &helium_entity_manager::id(),
    );

    let (bubblegum_signer, _bs_bump) =
        Pubkey::find_program_address(&["collection_cpi".as_bytes()], &BGUM_PID);
    Ok(IssueDataOnlyEntityV0 {
        payer: program.payer(),
        ecc_verifier: Pubkey::from_str(ECC_VERIFIER)?,
        collection: data_only_config_acc.collection,
        collection_metadata,
        collection_master_edition,
        data_only_config,
        entity_creator,
        dao: dao.key(),
        key_to_asset,
        tree_authority,
        recipient: program.payer(),
        merkle_tree: data_only_config_acc.merkle_tree,
        data_only_escrow,
        bubblegum_signer,
        token_metadata_program: token_metadata_pid,
        log_wrapper: noop_pid,
        bubblegum_program: BGUM_PID,
        compression_program: compression_pid,
        system_program: system_program::id(),
    })
}

fn construct_onboard_iot_accounts(
    program: &Program,
    entity_key: &[u8],
) -> Result<OnboardDataOnlyIotHotspotV0> {
    let compression_program = spl_account_compression::id();
    let dc_mint = Token::Dc.mint();
    let dao = Dao::Hnt;
    let sub_dao = SubDao::Iot;

    let rewardable_entity_config = sub_dao.rewardable_entity_config_key();

    let iot_info = sub_dao.info_key(rewardable_entity_config.as_ref())?;

    let data_only_config = dao.data_only_config_key();
    let data_only_config_acc = program
        .account::<DataOnlyConfigV0>(data_only_config)
        .map_err(|e| anyhow!("Couldn't find data only config: {e}"))?;

    let key_to_asset = dao.key_to_asset(entity_key);

    let dc = SubDao::dc_key();

    Ok(OnboardDataOnlyIotHotspotV0 {
        payer: program.payer(),
        dc_fee_payer: program.payer(),
        iot_info,
        hotspot_owner: program.payer(),
        merkle_tree: data_only_config_acc.merkle_tree,
        dc_burner: get_associated_token_address(&program.payer(), dc_mint),
        rewardable_entity_config,
        data_only_config,
        dao: dao.key(),
        key_to_asset,
        sub_dao: sub_dao.key(),
        dc_mint: *dc_mint,
        dc,
        compression_program,
        data_credits_program: DC_PID,
        token_program: anchor_spl::token::ID,
        associated_token_program: spl_associated_token_account::id(),
        system_program: system_program::id(),
    })
}
