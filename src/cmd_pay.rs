use crate::{
    hnt::Hnt,
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, B58},
    wallet::Wallet,
};
use helium_api::Client;
use helium_proto::txn::{TxnPaymentV1, Wrapper};
use prettytable::Table;
use prost::Message;
use sha2::{Digest, Sha256};

pub fn cmd_pay(wallet: &Wallet, password: &str, payee: String, amount: Hnt) -> Result {
    let client = Client::new();

    let keypair = wallet.to_keypair(password.as_bytes())?;
    let account = client.get_account(&keypair.public.to_b58()?)?;

    let mut txn = TxnPaymentV1 {
        amount: amount.to_bones(),
        fee: 0,
        payee: PubKeyBin::from_b58(payee)?.to_vec(),
        payer: keypair.pubkey_bin().to_vec(),
        nonce: account.nonce + 1,
        signature: Vec::new(),
    };
    txn.sign(&keypair)?;

    client.submit_txn(Wrapper::Payment(txn.clone()))?;
    print_txn(&txn);

    Ok(())
}

pub fn print_txn(txn: &TxnPaymentV1) {
    let mut txn_copy = txn.clone();
    // clear the signature so we can compute the hash
    txn_copy.signature = Vec::new();

    let mut hasher = Sha256::new();
    // write input message
    let mut buf = Vec::new();

    txn_copy.encode(&mut buf).unwrap();
    hasher.input(buf.as_slice());
    let result = hasher.result();

    let mut data = [0u8; 33];
    data[1..].copy_from_slice(&result);

    let mut table = Table::new();
    table.add_row(row!["Payee", "Amount", "Nonce", "Txn Hash"]);
    table.add_row(row![
        PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
        txn.amount,
        txn.nonce,
        bs58::encode(data.as_ref()).with_check().into_string()
    ]);
    table.printstd();
}
