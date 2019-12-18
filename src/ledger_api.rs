use ledger::*;

use super::*;

use helium_api::Client;

use super::traits::B58;
use crate::result::Result;
use keypair::PubKeyBin;
use prettytable::Table;

const INS_GET_PUBLIC_KEY: u8 = 0x02;
const INS_SIGN_PAYMENT_TXN: u8 = 0x08;

enum PubkeyDisplay {
    Off,
    On,
}

fn exchange_tx_get_pubkey(ledger: &LedgerApp, display: PubkeyDisplay) -> Result<PubKeyBin> {
    let get_public_key = ApduCommand {
        cla: 0xe0,
        ins: INS_GET_PUBLIC_KEY,
        p1: match display {
            PubkeyDisplay::On => 0x01,
            PubkeyDisplay::Off => 0x00,
        },
        p2: 0x00,
        length: 0,
        data: Vec::new(),
    };

    let public_key_result = ledger.exchange(get_public_key)?;
    // TODO: verify validity before returning by checking the sha256 checksum
    Ok(PubKeyBin::from_vec(&public_key_result.data[1..34]))
}

pub fn load_wallet() -> Result {
    let ledger = LedgerApp::new()?;
    let keypair = exchange_tx_get_pubkey(&ledger, PubkeyDisplay::On)?;

    let mut table = Table::new();
    table.add_row(row!["Address", "Type"]);
    table.add_row(row![keypair.to_b58()?, "Ledger"]);
    table.printstd();

    Ok(())
}

fn transform_u64_to_array_of_u8(x: u64) -> [u8; 8] {
    let b8: u8 = ((x >> 56) & 0xff) as u8;
    let b7: u8 = ((x >> 48) & 0xff) as u8;
    let b6: u8 = ((x >> 40) & 0xff) as u8;
    let b5: u8 = ((x >> 32) & 0xff) as u8;
    let b4: u8 = ((x >> 24) & 0xff) as u8;
    let b3: u8 = ((x >> 16) & 0xff) as u8;
    let b2: u8 = ((x >> 8) & 0xff) as u8;
    let b1: u8 = (x & 0xff) as u8;
    return [b1, b2, b3, b4, b5, b6, b7, b8];
}

use super::cmd_pay::print_txn;
use helium_proto::txn::{TxnPaymentV1, Wrapper};
use prost::Message;

pub fn pay(payee: String, amount: u64) -> Result {
    let ledger = LedgerApp::new()?;
    let client = Client::new();
    let fee: u64 = 0;
    let mut data: Vec<u8> = Vec::new();

    println!("Communicating with Ledger - confirm it is unlocked and follow prompts on screen");
    // get nonce
    let keypair = exchange_tx_get_pubkey(&ledger, PubkeyDisplay::Off)?;
    let account = client.get_account(&keypair.to_b58()?)?;
    let nonce: u64 = account.nonce + 1;

    if account.balance < amount {
        println!(
            "Account balance insufficient. {} Bones on account but attempting to send {}",
            account.balance, amount
        );
        return Ok(());
    }

    // serlialize payee
    let payee_bin = PubKeyBin::from_b58(payee)?.to_vec();
    data.extend(&transform_u64_to_array_of_u8(amount));
    data.extend(&transform_u64_to_array_of_u8(fee));
    data.extend(&transform_u64_to_array_of_u8(nonce));

    data.push(0);
    data.extend(payee_bin.as_slice());

    let exchange_pay_tx = ApduCommand {
        cla: 0xe0,
        ins: INS_SIGN_PAYMENT_TXN,
        p1: 0x00,
        p2: 0x00,
        length: 0,
        data,
    };

    let exchange_pay_tx_result = ledger.exchange(exchange_pay_tx)?;

    if exchange_pay_tx_result.data.len() == 0 {
        println!("Transaction not confirmed");
        return Ok(());
    }

    let txn = TxnPaymentV1::decode(exchange_pay_tx_result.data.clone())?;

    client.submit_txn(Wrapper::Payment(txn.clone()))?;

    print_txn(&txn);
    Ok(())
}

pub fn get_address() -> Result<Vec<String>> {
    let ledger = LedgerApp::new()?;
    let mut ret: Vec<String> = Vec::new();
    // get nonce
    let keypair = exchange_tx_get_pubkey(&ledger, PubkeyDisplay::Off)?;
    ret.push(keypair.to_b58()?);
    Ok(ret)
}
