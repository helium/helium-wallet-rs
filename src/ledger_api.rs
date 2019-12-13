use ledger::*;

use super::*;
use ascii::AsAsciiStr;
use keypair::Keypair;
use wallet::BasicFormat;

use helium_api::Client;

use super::traits::{ReadWrite, B58};
use bs58;
use crate::result::Result;
use prettytable::Table;
use keypair::PubKeyBin;

const INS_GET_PUBLIC_KEY: u8 = 0x02;
const INS_GET_TXN_HASH: u8 = 0x08;

enum PubkeyDisplay {
    Off,
    On
}

fn exchange_tx_get_pubkey(ledger: &LedgerApp, display: PubkeyDisplay) -> Result<PubKeyBin> {
    let get_public_key = ApduCommand {
        cla: 0xe0,
        ins: INS_GET_PUBLIC_KEY,
        p1: match display {
            PubkeyDisplay::On => 0x01,
            PubkeyDisplay::Off => 0x00
        },
        p2: 0x00,
        length: 0,
        data: Vec::new()
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

fn transform_u64_to_array_of_u8(x:u64) -> [u8;8] {
    let b8 : u8 = ((x >> 56) & 0xff) as u8;
    let b7 : u8 = ((x >> 48) & 0xff) as u8;
    let b6 : u8 = ((x >> 40) & 0xff) as u8;
    let b5 : u8 = ((x >> 32) & 0xff) as u8;
    let b4 : u8 = ((x >> 24) & 0xff) as u8;
    let b3 : u8 = ((x >> 16) & 0xff) as u8;
    let b2 : u8 = ((x >> 8) & 0xff) as u8;
    let b1 : u8 = (x & 0xff) as u8;
    return [b1, b2, b3, b4, b5, b6, b7, b8]
}
use bytes::Bytes;
use helium_proto::txn::{Txn, TxnPaymentV1, Wrapper};
use prost::Message;

//use bytes::buf::buf_mut::BufMut;

pub fn pay(payee: String, amount: u64) -> Result {
    let mut vec : Vec<u8> = Vec::new();
    vec.extend_from_slice(&[
        0xa, 0x21, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x12, 0x21, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x18, 0x37, 0x20, 0x20, 0x28, 0x16, 0x32, 0x20, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    ]);
    let mut buf = Bytes::from(vec);
    println!("buf {:?}", buf);
    //let bytes = b"Hjello";
    let tx = TxnPaymentV1::decode(buf);
    println!("tx = {:?}", tx);
    
    //let mut buf = bytes.into_buf();
    //let result =  TxnPaymentV1::decode(&buf)?;
    // if let TxnPaymentV1::Binary(ref v) = msg {
    //   let response = Response::decode(v);
    // }
    //println!("{:?}", result);
    //println!("payment amount {:?} ", amount);

    let ledger = LedgerApp::new()?;
    let client = Client::new();
    let fee: u64 = 0;
    let mut data: Vec<u8> = Vec::new();

    // get nonce
    let keypair = exchange_tx_get_pubkey(&ledger, PubkeyDisplay::Off)?;
    let account = client.get_account(&keypair.to_b58()?)?;
    let nonce: u64 = account.nonce + 1;

    // serlialize payee
    let payee_bin = PubKeyBin::from_b58(payee)?.to_vec();
    println!("bin [{}] {:?} ", payee_bin.len(), payee_bin);

    data.extend(&transform_u64_to_array_of_u8(amount));
    data.extend(&transform_u64_to_array_of_u8(fee));
    data.extend(&transform_u64_to_array_of_u8(nonce));

    // TODO: add wallet checksum at end (in binary)
    data.push(0); // prepend with 0
    data.extend(payee_bin.as_slice());
    println!("data {:?} ", data);

    let mut exchange_pay_tx = ApduCommand {
        cla: 0xe0,
        ins: INS_GET_TXN_HASH,
        p1: 0x00,
        p2: 0x00,
        length: 0,
        data
    };

    let exchange_pay_tx_result = ledger.exchange(exchange_pay_tx)?;
    println!("{:?}", exchange_pay_tx_result.data);
    Ok(())
}
