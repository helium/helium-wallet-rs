use crate::{cmd::*, result::Result, traits::txn_envelope::TxnEnvelope};
use anchor_client::Program;
use bs58;
use data_credits::ID as DC_PID;
use helium_entity_manager::{
    accounts::{IssueDataOnlyEntityV0, OnboardDataOnlyIotHotspotV0},
    DataOnlyConfigV0, IssueDataOnlyEntityArgsV0, KeyToAssetV0, OnboardDataOnlyIotHotspotArgsV0,
    ECC_VERIFIER, ID as HEM_PID,
};
use helium_sub_daos::ID as HSD_PID;
use mpl_bubblegum::ID as BGUM_PID;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solana_program::system_program;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction, transaction::Transaction as SolanaTransaction,
};
use spl_associated_token_account::get_associated_token_address;
use std::str::FromStr;
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

const HNT_MINT: &str = "hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux";
const TOKEN_METADATA: &str = "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s";
const NOOP: &str = "noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV";
const COMPRESSION: &str = "cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK";
const IOT_MINT: &str = "iotEVVZLEywoTn1QdwNPddxPWszn3zFhEot3MfL9fns";
const DC_MINT: &str = "dcuc8Amr83Wz27ZkQ2K9NS6r8zRpf1J6cvArEBDZDmm";

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

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let add_gateway_txn =
            BlockchainTxnAddGatewayV1::from_envelope(&read_txn(&self.add_gateway_txn)?)?;
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let anchor_client = client.settings.mk_anchor_client(keypair.clone())?;

        let program = anchor_client.program(helium_sub_daos::id());

        let default_url = &"https://ecc-verifier.web.helium.io".to_string();
        let ecc_verifier_url = match &self.ecc_verifier_url {
            Some(url) => url,
            None => default_url,
        };
        let entity_key = add_gateway_txn.gateway.clone();

        println!("Issuing entity");
        let hash = Sha256::digest(entity_key.clone());

        // check if entity has been issued by checking key_to_asset exists
        let (dao, _dao_bump) = Pubkey::find_program_address(
            &[
                "dao".as_bytes(),
                Pubkey::from_str(HNT_MINT).unwrap().as_ref(),
            ],
            &HSD_PID,
        );
        println!("DAO: {}", dao.to_string());
        let (key_to_asset, _kta_bump) = Pubkey::find_program_address(
            &["key_to_asset".as_bytes(), dao.as_ref(), &hash],
            &HEM_PID,
        );

        let kta = program.rpc().get_account(&key_to_asset);
        // If the entity has not been issued, issue it. Otherwise, onboard it.
        if kta.is_err() {
            // construct the issue entity transaction
            let issue_entity_accounts = construct_issue_entity_accounts(
                &program,
                Pubkey::from_str(HNT_MINT).unwrap(),
                &entity_key,
            );
            let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(200000);
            let tx = program
                .request()
                .args(helium_entity_manager::instruction::IssueDataOnlyEntityV0 {
                    args: IssueDataOnlyEntityArgsV0 {
                        entity_key: entity_key.clone(),
                    },
                })
                .accounts(issue_entity_accounts)
                .instruction(compute_ix)
                .signed_transaction()
                .unwrap();

            let serialized_tx = hex::encode(&bincode::serialize(&tx).unwrap());
            // verify the base64 transaction with the ecc-sig-verifier
            let req_client = reqwest::blocking::Client::new();
            let response = req_client
                .post(ecc_verifier_url)
                .json(&VerifyRequest {
                    transaction: &serialized_tx,
                    msg: &hex::encode(bincode::serialize(&add_gateway_txn).unwrap()),
                    signature: &hex::encode(add_gateway_txn.gateway_signature),
                })
                .send()
                .unwrap()
                .json::<VerifyResponse>()
                .unwrap();
            let raw_signed_tx = hex::decode(response.transaction).unwrap();
            let signed_tx: SolanaTransaction = bincode::deserialize(&raw_signed_tx).unwrap();

            program
                .rpc()
                .send_and_confirm_transaction_with_spinner(&signed_tx)
                .unwrap();
        } else {
            println!("Entity already issued");
        }

        // return Ok(());

        println!("Onboarding hotspot");

        let kta_acc = program.account::<KeyToAssetV0>(key_to_asset).unwrap();
        let issue_entity_accounts = construct_onboard_iot_accounts(
            &program,
            Pubkey::from_str(HNT_MINT).unwrap(),
            &entity_key,
        );
        let compute_ix = ComputeBudgetInstruction::set_compute_unit_limit(200000);

        let req_client = reqwest::blocking::Client::new();
        let get_asset_response = req_client
            .post(program.rpc().url())
            .body(format!(
                "{{
                jsonrpc: '2.0',
                method: 'getAsset',
                id: 'rpd-op-123',
                params: {{ id: {} }},
                headers: {{
                  'Cache-Control': 'no-cache',
                  Pragma: 'no-cache',
                  Expires: '0',
                }},
              }}",
                kta_acc.asset.to_string(),
            ))
            .send()
            .unwrap()
            .json::<serde_json::Value>()
            .unwrap();
        let data_hash: [u8; 32] = bs58::decode(
            get_asset_response.as_object().unwrap()["result"]["compression"]["data_hash"]
                .as_str()
                .unwrap(),
        )
        .into_vec()
        .unwrap()
        .as_slice()
        .try_into()
        .unwrap();
        let creator_hash: [u8; 32] = bs58::decode(
            get_asset_response.as_object().unwrap()["result"]["compression"]["creator_hash"]
                .as_str()
                .unwrap(),
        )
        .into_vec()
        .unwrap()
        .as_slice()
        .try_into()
        .unwrap();
        let leaf_id = get_asset_response.as_object().unwrap()["result"]["compression"]["leaf_id"]
            .as_u64()
            .unwrap();

        let req_client = reqwest::blocking::Client::new();
        let get_asset_proof_response = req_client
            .post(program.rpc().url())
            .body(format!(
                "{{
                jsonrpc: '2.0',
                method: 'getAssetProof',
                id: 'rpd-op-123',
                params: {{ id: {} }},
                headers: {{
                  'Cache-Control': 'no-cache',
                  Pragma: 'no-cache',
                  Expires: '0',
                }},
              }}",
                kta_acc.asset.to_string(),
            ))
            .send()
            .unwrap()
            .json::<serde_json::Value>()
            .unwrap();

        let root: [u8; 32] = Pubkey::from_str(
            get_asset_proof_response.as_object().unwrap()["result"]["proof"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
        .to_bytes();
        let tx = program
            .request()
            .args(
                helium_entity_manager::instruction::OnboardDataOnlyIotHotspotV0 {
                    args: OnboardDataOnlyIotHotspotArgsV0 {
                        data_hash,
                        creator_hash,
                        root,
                        index: leaf_id.try_into().unwrap(),
                        location: self.location,
                        elevation: self.elevation,
                        gain: self.gain,
                    },
                },
            )
            .accounts(issue_entity_accounts)
            .instruction(compute_ix)
            .signed_transaction()?;

        program.rpc().send_and_confirm_transaction(&tx)?;

        Ok(())
    }
}

fn construct_issue_entity_accounts(
    program: &Program,
    hnt_mint: Pubkey,
    entity_key: &Vec<u8>,
) -> IssueDataOnlyEntityV0 {
    let token_metadata_pid = Pubkey::from_str(TOKEN_METADATA).unwrap();
    let noop_pid = Pubkey::from_str(NOOP).unwrap();
    let compression_pid = Pubkey::from_str(COMPRESSION).unwrap();
    let (dao, _dao_bump) =
        Pubkey::find_program_address(&["dao".as_bytes(), hnt_mint.as_ref()], &HEM_PID);

    let (data_only_config, _data_only_bump) =
        Pubkey::find_program_address(&["data_only_config".as_bytes(), dao.as_ref()], &HEM_PID);

    let data_only_config_acc = program
        .account::<DataOnlyConfigV0>(data_only_config)
        .unwrap();

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

    let (entity_creator, _ec_bump) =
        Pubkey::find_program_address(&["entity_creator".as_bytes(), dao.as_ref()], &HEM_PID);

    // get the sha256 hash of the entity_key
    let hash = Sha256::digest(entity_key.clone());

    let (key_to_asset, _kta_bump) =
        Pubkey::find_program_address(&["key_to_asset".as_bytes(), dao.as_ref(), &hash], &HEM_PID);

    let (tree_authority, _ta_bump) =
        Pubkey::find_program_address(&[data_only_config_acc.merkle_tree.as_ref()], &BGUM_PID);

    let (data_only_escrow, _doe_bump) = Pubkey::find_program_address(
        &["data_only_escrow".as_bytes(), data_only_config.as_ref()],
        &HEM_PID,
    );

    let (bubblegum_signer, _bs_bump) =
        Pubkey::find_program_address(&["collection_cpi".as_bytes()], &BGUM_PID);
    IssueDataOnlyEntityV0 {
        payer: program.payer(),
        ecc_verifier: Pubkey::from_str(ECC_VERIFIER).unwrap(),
        collection: data_only_config_acc.collection,
        collection_metadata,
        collection_master_edition,
        data_only_config,
        entity_creator,
        dao,
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
    }
}

fn construct_onboard_iot_accounts(
    program: &Program,
    hnt_mint: Pubkey,
    entity_key: &Vec<u8>,
) -> OnboardDataOnlyIotHotspotV0 {
    let compression_program = Pubkey::from_str(COMPRESSION).unwrap();
    let iot_mint = Pubkey::from_str(IOT_MINT).unwrap();
    let dc_mint = Pubkey::from_str(DC_MINT).unwrap();

    let (dao, _dao_bump) =
        Pubkey::find_program_address(&["dao".as_bytes(), hnt_mint.as_ref()], &HEM_PID);
    let (sub_dao, _sd_bump) =
        Pubkey::find_program_address(&["sub_dao".as_bytes(), iot_mint.as_ref()], &HSD_PID);

    let (rewardable_entity_config, _rec_bump) = Pubkey::find_program_address(
        &[
            "rewardable_entity_config".as_bytes(),
            sub_dao.as_ref(),
            "IOT".as_bytes(),
        ],
        &HEM_PID,
    );

    let hash = Sha256::digest(entity_key.clone());

    let (iot_info, _info_bump) = Pubkey::find_program_address(
        &[
            "iot_info".as_bytes(),
            rewardable_entity_config.as_ref(),
            &hash,
        ],
        &HEM_PID,
    );

    let (data_only_config, _data_only_bump) =
        Pubkey::find_program_address(&["data_only_config".as_bytes(), dao.as_ref()], &HEM_PID);

    let data_only_config_acc = program
        .account::<DataOnlyConfigV0>(data_only_config)
        .unwrap();

    let (key_to_asset, _kta_bump) =
        Pubkey::find_program_address(&["key_to_asset".as_bytes(), dao.as_ref(), &hash], &HEM_PID);

    let (dc, _dc_bump) =
        Pubkey::find_program_address(&["dc".as_bytes(), dc_mint.as_ref()], &DC_PID);

    OnboardDataOnlyIotHotspotV0 {
        payer: program.payer(),
        dc_fee_payer: program.payer(),
        iot_info,
        hotspot_owner: program.payer(),
        merkle_tree: data_only_config_acc.merkle_tree,
        dc_burner: get_associated_token_address(&program.payer(), &dc_mint),
        rewardable_entity_config,
        data_only_config,
        dao,
        key_to_asset,
        sub_dao,
        dc_mint,
        dc,
        compression_program,
        data_credits_program: DC_PID,
        token_program: anchor_spl::token::ID,
        associated_token_program: spl_associated_token_account::id(),
        system_program: system_program::id(),
    }
}
