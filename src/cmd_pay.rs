use crate::{result::Result, wallet::Wallet};
use helium_proto::txns::blockchain::*;
use prost::Message;

pub fn cmd_pay(wallet: &Wallet, password: &str, payee: String, amount: u64) -> Result {
    let keypair = wallet.to_keypair(password.as_bytes())?;

    let payment = TxnPaymentV1 {
        amount,
        fee: 0,
        payee: payee.as_bytes().to_vec(),
        payer: Vec::new(),
        nonce: 1,
        signature: Vec::new(),
    };
    let mut buf = vec![];
    payment.encode(&mut buf)?;
    let _signature = keypair.sign(&buf);

    Ok(())
}
