use crate::{
    cmd::{
        api_url, get_password, get_txn_fees, load_wallet, print_footer, print_json, print_table,
        status_json, status_str, Opts, OutputFormat,
    },
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, TxnEnvelope, TxnFee, B58, B64},
};
use helium_api::{BlockchainTxn, BlockchainTxnPaymentV2, Client, Hnt, Payment, PendingTxnStatus};
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

        let keypair = wallet.decrypt(password.as_bytes())?;
        let account = client.get_account(&keypair.public.to_b58()?)?;

        let payments: Result<Vec<Payment>> = self
            .payees
            .iter()
            .map(|p| {
                Ok(Payment {
                    payee: PubKeyBin::from_b58(&p.address)?.into(),
                    amount: p.amount.to_bones(),
                })
            })
            .collect();
        let mut txn = BlockchainTxnPaymentV2 {
            fee: 0,
            payments: payments?,
            payer: keypair.pubkey_bin().into(),
            nonce: account.speculative_nonce + 1,
            signature: Vec::new(),
        };
        txn.fee = txn.txn_fee(&get_txn_fees(&client)?)?;
        txn.signature = txn.sign(&keypair)?;
        let envelope = txn.in_envelope();
        let status = if self.commit {
            Some(client.submit_txn(&envelope)?)
        } else {
            None
        };

        print_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnPaymentV2,
    envelope: &BlockchainTxn,
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
            print_table(&table)?;

            ptable!(
                ["Key", "Value"],
                ["Fee", txn.fee],
                ["Nonce", txn.nonce],
                ["Hash", status_str(status)]
            );

            print_footer(status)
        }
        OutputFormat::Json => {
            let mut payments = Vec::with_capacity(txn.payments.len());
            for payment in txn.payments.clone() {
                payments.push(json!({
                    "payee": PubKeyBin::from_vec(&payment.payee).to_b58().unwrap(),
                    "amount": Hnt::from_bones(payment.amount),
                }))
            }
            let table = json!({
                "payments": payments,
                "fee": txn.fee,
                "nonce": txn.nonce,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
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
