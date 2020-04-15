use crate::{
    cmd::{api_url, get_password, load_wallet, Opts, OutputFormat},
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, B58},
};
use helium_api::{Client, Hnt, PendingTxnStatus};
use helium_proto::{BlockchainTxnPaymentV2, Payment, Txn};
use prettytable::Table;
use serde_json::json;
use std::str::FromStr;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Send one or more payments to given addresses. Note that HNT only
/// goes to 8 decimals of precision. The payment is not submitted to
/// the system unless the '--commit' option is given.
pub struct Cmd {
    /// Address and amount of HNT to send in <address>=<amount> format.
    #[structopt(long = "payee", short = "p", name = "payee=hnt", required = true)]
    payees: Vec<Payee>,

    /// Commit the payment to the API
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;

        let client = Client::new_with_base_url(api_url());

        let keypair = wallet.to_keypair(password.as_bytes())?;
        let account = client.get_account(&keypair.public.to_b58()?)?;

        let payments: Result<Vec<Payment>> = self
            .payees
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

        let status = if self.commit {
            Some(client.submit_txn(wrapped_txn.clone())?)
        } else {
            None
        };

        print_txn(&txn, wrapped_txn, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnPaymentV2,
    wrapped_txn: Txn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
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

            Ok(())
        }
        OutputFormat::Json => {
            let mut payments = Vec::with_capacity(txn.payments.len());
            for payment in txn.payments.clone() {
                payments.push(json!({
                    "payee": PubKeyBin::from_vec(&payment.payee).to_b58().unwrap(),
                    "amount": Hnt::from_bones(payment.amount),
                }))
            }
            let table = if status.is_some() {
                json!({
                    "payments": payments,
                    "nonce": txn.nonce,
                    "hash": status.as_ref().map_or("none", |s| &s.hash),
                    "txn": Client::txn_to_b64(wrapped_txn)?,

                })
            } else {
                json!({
                    "payments": payments,
                    "txn": Client::txn_to_b64(wrapped_txn)?,
                })
            };
            println!("{}", serde_json::to_string_pretty(&table)?);
            Ok(())
        }
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
            amount: s[pos + 1..].parse()?,
        })
    }
}
