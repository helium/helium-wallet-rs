use crate::{
    keypair::{PubKeyBin, Keypair},
    result::Result,
    traits::{Sign, B58},
    wallet::Wallet,
};
use helium_api::{Client, PendingTxnStatus};
use helium_proto::{BlockchainTxnCreateHtlcV1, BlockchainTxnRedeemHtlcV1, Txn};
use prettytable::Table;

pub fn cmd_create(
    url: String,
    wallet: &Wallet,
    password: &str,
    payee: String,    
    hashlock: String,
    timelock: u64,
    amount: u64,
    commit: bool,
    hash: bool,
) -> Result {
    let client = Client::new_with_base_url(url);

    let keypair = wallet.to_keypair(password.as_bytes())?;
    let account = client.get_account(&keypair.public.to_b58()?)?;
    let address = Keypair::gen_keypair().pubkey_bin();

    let mut txn = BlockchainTxnCreateHtlcV1 {
        amount,
        fee: 0,
        payee: PubKeyBin::from_b58(payee)?.to_vec(),
        payer: keypair.pubkey_bin().to_vec(),
        address: address.to_vec(),
        hashlock: hex::decode(hashlock).unwrap(),
        timelock,
        nonce: account.speculative_nonce + 1,
        signature: Vec::new(),
    };
    txn.sign(&keypair)?;
    let wrapped_txn = Txn::CreateHtlc(txn.clone());

    let status = if commit {
        Some(client.submit_txn(wrapped_txn)?)
    } else {
        None
    };

    if hash {
        println!("{}", status.map_or("none".to_string(), |s| s.hash));
    } else {
        print_create_txn(&txn, &status);
    }

    Ok(())
}

fn print_create_txn(txn: &BlockchainTxnCreateHtlcV1, status: &Option<PendingTxnStatus>) {
    let mut table = Table::new();
    table.add_row(row!["Address", "Payee", "Amount", "Hashlock", "Timelock"]);
    table.add_row(row![
        PubKeyBin::from_vec(&txn.address).to_b58().unwrap(),
        PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
        txn.amount,
        hex::encode(&txn.hashlock),
        txn.timelock
    ]);
    table.printstd();

    if status.is_some() {
        ptable!(
            ["Nonce", "Hash"],
            [txn.nonce, status.as_ref().map_or("none", |s| &s.hash)]
        );
    }
}

pub fn cmd_redeem(
    url: String,
    wallet: &Wallet,
    password: &str,
    address: String,
    preimage: String,
    hash: bool,
) -> Result {
    let client = Client::new_with_base_url(url);

    let keypair = wallet.to_keypair(password.as_bytes())?;

    let mut txn = BlockchainTxnRedeemHtlcV1 {
        fee: 0,
        payee: keypair.pubkey_bin().to_vec(),
        address: PubKeyBin::from_b58(address)?.to_vec(),
        preimage: preimage.into_bytes(),
        signature: Vec::new(),
    };
    txn.sign(&keypair)?;
    let wrapped_txn = Txn::RedeemHtlc(txn.clone());

    let status = client.submit_txn(wrapped_txn)?;   

    if hash {
        println!("{}", status.hash);
    } else {
        print_redeem_txn(&txn, &status);
    }

    Ok(())
}

fn print_redeem_txn(txn: &BlockchainTxnRedeemHtlcV1, status: &PendingTxnStatus) {
    let mut table = Table::new();
    table.add_row(row!["Payee", "Address", "Preimage", "Hash"]);
    table.add_row(row![
        PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
        PubKeyBin::from_vec(&txn.address).to_b58().unwrap(),
        std::str::from_utf8(&txn.preimage).unwrap(),
        status.hash
    ]);
    table.printstd();
}
