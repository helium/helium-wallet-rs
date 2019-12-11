use ledger::*;

use super::*;
use ascii::AsAsciiStr;
use keypair::Keypair;
use wallet::BasicFormat;

// The APDU protocol uses a single-byte instruction code (INS) to specify
// which command should be executed. We'll use this code to dispatch on a
// table of function pointers.
/*
#define INS_GET_VERSION    0x01
#define INS_GET_PUBLIC_KEY 0x02
#define INS_SIGN_HASH      0x04
#define INS_GET_TXN_HASH   0x08
*/
use super::traits::{ReadWrite, B58};
use bs58;
use crate::result::Result;
use prettytable::Table;

const INS_GET_PUBLIC_KEY: u8 = 0x02;

pub fn load_wallet() -> Result {
    
    let ledger = LedgerApp::new()?;
    let get_public_key = ApduCommand {
        cla: 0xe0,
        ins: INS_GET_PUBLIC_KEY,
        p1: 0x00,
        p2: 0x00,
        length: 0,
        data: Vec::new()
    };

    let public_key_result = ledger.exchange(get_public_key)?;
    println!("{:?}", public_key_result);

    let keypair = keypair::PubKeyBin::from_vec(&public_key_result.data[1..34]);

    let mut table = Table::new();
    table.add_row(row!["Address", "Type"]);
    table.add_row(row![keypair.to_b58()?, "Ledger"]);
    table.printstd();

    Ok(())
}

pub fn pay(payee: String, amount: u64) -> Result {
    
    let fee: u64 = 0;



    Ok(())
}
