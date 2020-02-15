use crate::{
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, B58},
    wallet::Wallet,
};
use helium_api::{Client, PendingTxnStatus};
use helium_proto::{BlockchainTxnPaymentV1, Txn};
use prettytable::Table;

pub fn cmd_pay(
    url: String,
    wallet: &Wallet,
    password: &str,
    payee: String,
    amount: u64,
    hash: bool,
) -> Result {
    let client = Client::new_with_base_url(url);

    let keypair = wallet.to_keypair(password.as_bytes())?;
    let account = client.get_account(&keypair.public.to_b58()?)?;

    let mut txn = BlockchainTxnPaymentV1 {
        amount,
        fee: 0,
        payee: PubKeyBin::from_b58(payee)?.to_vec(),
        payer: keypair.pubkey_bin().to_vec(),
        nonce: account.speculative_nonce + 1,
        signature: Vec::new(),
    };
    txn.sign(&keypair)?;
    let wrapped_txn = Txn::Payment(txn.clone());

    let status = client.submit_txn(wrapped_txn)?;

    if hash {
        println!("{}", status.hash);
    } else {
        print_txn(&txn, &status);
    }

    Ok(())
}

fn print_txn(txn: &BlockchainTxnPaymentV1, status: &PendingTxnStatus) {
    let mut table = Table::new();
    table.add_row(row!["Payee", "Amount", "Nonce", "Hash"]);
    table.add_row(row![
        PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
        txn.amount,
        txn.nonce,
        status.hash
    ]);
    table.printstd();
}
