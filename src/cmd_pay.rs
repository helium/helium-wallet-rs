use crate::{
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, B58},
    wallet::Wallet,
};
use helium_api::Client;
use helium_proto::{BlockchainTxnPaymentV1, Txn};
use prettytable::Table;

pub fn cmd_pay(wallet: &Wallet, password: &str, payee: String, amount: u64) -> Result {
    let client = Client::default();

    let keypair = wallet.to_keypair(password.as_bytes())?;
    let account = client.get_account(&keypair.public.to_b58()?)?;

    let mut txn = BlockchainTxnPaymentV1 {
        amount,
        fee: 0,
        payee: PubKeyBin::from_b58(payee)?.to_vec(),
        payer: keypair.pubkey_bin().to_vec(),
        nonce: account.nonce + 1,
        signature: Vec::new(),
    };
    txn.sign(&keypair)?;

    client.submit_txn(Txn::Payment(txn.clone()))?;
    print_txn(&txn);

    Ok(())
}

fn print_txn(txn: &BlockchainTxnPaymentV1) {
    let mut table = Table::new();
    table.add_row(row!["Payee", "Amount", "Nonce"]);
    table.add_row(row![PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(), txn.amount, txn.nonce]);
    table.printstd();
}
