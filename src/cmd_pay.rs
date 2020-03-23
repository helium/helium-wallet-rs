use crate::{
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, B58},
    wallet::Wallet,
};
use helium_api::{Client, Hnt, PendingTxnStatus};
use helium_proto::{BlockchainTxnPaymentV2, Payment, Txn};
use prettytable::Table;
use std::str::FromStr;

pub fn cmd_pay(
    url: String,
    wallet: &Wallet,
    password: &str,
    payees: Vec<Payee>,
    commit: bool,
    hash: bool,
) -> Result {
    let client = Client::new_with_base_url(url);

    let keypair = wallet.to_keypair(password.as_bytes())?;
    let account = client.get_account(&keypair.public.to_b58()?)?;

    let payments: Result<Vec<Payment>> = payees
        .iter()
        .map(|p| {
            Ok(Payment {
                payee: PubKeyBin::from_b58(p.address.clone())?.to_vec(),
                amount: p.amount.to_bones(),
            })
        })
        .collect();
    let mut txn = BlockchainTxnPaymentV2 {
        fee: 0,
        payments: payments?,
        payer: keypair.pubkey_bin().to_vec(),
        nonce: account.speculative_nonce + 1,
        signature: Vec::new(),
    };
    txn.sign(&keypair)?;
    let wrapped_txn = Txn::PaymentV2(txn.clone());

    let status = if commit {
        Some(client.submit_txn(wrapped_txn)?)
    } else {
        None
    };

    if hash {
        println!("{}", status.map_or("none".to_string(), |s| s.hash));
    } else {
        print_txn(&txn, &status);
    }

    Ok(())
}

fn print_txn(txn: &BlockchainTxnPaymentV2, status: &Option<PendingTxnStatus>) {
    let mut table = Table::new();
    table.add_row(row!["Payee", "Amount"]);
    for payment in txn.payments.clone() {
        table.add_row(row![
            PubKeyBin::from_vec(&payment.payee).to_b58().unwrap(),
            Hnt::from_bones(payment.amount)
        ]);
    }
    table.printstd();

    if status.is_some() {
        ptable!(
            ["Nonce", "Hash"],
            [txn.nonce, status.as_ref().map_or("none", |s| &s.hash)]
        );
    }
}

#[derive(Debug)]
pub struct Payee {
    address: String,
    amount: Hnt,
}

impl FromStr for Payee {
    type Err = Box<dyn std::error::Error>;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let pos = s
            .find('=')
            .ok_or_else(|| format!("invalid KEY=value: missing `=`  in `{}`", s))?;
        Ok(Payee {
            address: s[..pos].to_string(),
            amount: s[pos + 2..].parse()?,
        })
    }
}
