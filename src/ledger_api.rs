use ledger::*;

use super::*;

use helium_api::Client;

use super::traits::B58;
use crate::result::Result;
use byteorder::{LittleEndian as LE, WriteBytesExt};
use keypair::PubKeyBin;
use prettytable::Table;
use std::error;
use std::fmt;
const INS_GET_PUBLIC_KEY: u8 = 0x02;
const INS_SIGN_PAYMENT_TXN: u8 = 0x08;

// This parameter indicates whether the ledgers screen display the public key or not
// Thus, the `pay` function can do the Adpu transaction quietly to get the public key
enum PubkeyDisplay {
    Off = 0,
    On = 1,
}

#[derive(Debug)]
enum Error {
    CouldNotFindLedger,
    AppNotRunning,
}

fn exchange_tx_get_pubkey(ledger: &LedgerApp, display: PubkeyDisplay) -> Result<PubKeyBin> {
    let get_public_key = ApduCommand {
        cla: 0xe0,
        ins: INS_GET_PUBLIC_KEY,
        p1: display as u8,
        p2: 0x00,
        length: 0,
        data: Vec::new(),
    };

    let public_key_result = read_from_ledger(ledger, get_public_key)?;
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

use super::cmd_pay::print_txn;
use helium_proto::txn::{TxnPaymentV1, Wrapper};
use prost::Message;

pub fn pay(payee: String, amount: u64) -> Result {
    let ledger = LedgerApp::new()?;
    let client = Client::new();
    let fee: u64 = 0;
    let mut data: Vec<u8> = Vec::new();

    println!("Communicating with Ledger - follow prompts on screen");
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
    let payee_bin = PubKeyBin::from_b58(payee)?;
    data.write_u64::<LE>(amount)?;
    data.write_u64::<LE>(fee)?;
    data.write_u64::<LE>(nonce)?;

    data.push(0);
    data.extend(payee_bin.0.iter());

    let exchange_pay_txn = ApduCommand {
        cla: 0xe0,
        ins: INS_SIGN_PAYMENT_TXN,
        p1: 0x00,
        p2: 0x00,
        length: 0,
        data,
    };

    let exchange_pay_tx_result = read_from_ledger(&ledger, exchange_pay_txn)?;

    if exchange_pay_tx_result.data.len() == 1 {
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

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::CouldNotFindLedger => {
                write!(f, "Could not find ledger. Is it disconnected or locked?")
            }
            Error::AppNotRunning => write!(
                f,
                "Ledger is running but Helium application does not appear to be running"
            ),
        }
    }
}

fn read_from_ledger(
    ledger: &LedgerApp,
    command: ApduCommand,
) -> std::result::Result<ledger::ApduAnswer, Error> {
    let exchange = ledger.exchange(command);

    if let Ok(answer) = exchange {
        if answer.data.len() == 0 {
            Err(Error::AppNotRunning)
        } else {
            Ok(answer)
        }
    } else {
        Err(Error::CouldNotFindLedger)
    }
}
