use crate::{
    cmd::*,
    keypair::PublicKey,
    memo::Memo,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, B64},
};
use helium_api::accounts;
use prettytable::Table;
use serde_json::json;
use std::str::FromStr;

#[derive(Debug, StructOpt)]
/// Send one or more payments to given addresses. Note that HNT only
/// goes to 8 decimals of precision. The payment is not submitted to
/// the system unless the '--commit' option is given.
pub struct Cmd {
    /// Address and amount of HNT to sent in <address>?amount=<amount>?memo=<memo> format.
    /// Memo parameter is optional and may be ommitted.
    #[structopt(
        long = "payee",
        short = "p",
        name = "payee?<amount>=hnt?memo=<memo>",
        required = true
    )]
    payees: Vec<Payee>,

    /// Manually set the nonce to use for the transaction
    #[structopt(long)]
    nonce: Option<u64>,

    /// Manually set the DC fee to pay for the transaction
    #[structopt(long)]
    fee: Option<u64>,

    /// Commit the payment to the API
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;

        let client = Client::new_with_base_url(api_url(wallet.public_key.network));

        let keypair = wallet.decrypt(password.as_bytes())?;

        let payments: Vec<Payment> = self
            .payees
            .iter()
            .map(|p| Payment {
                payee: p.address.to_vec(),
                amount: u64::from(p.amount),
                memo: *p.memo.as_ref(),
            })
            .collect();

        let mut txn = BlockchainTxnPaymentV2 {
            fee: 0,
            payments,
            payer: keypair.public_key().to_vec(),
            nonce: if let Some(nonce) = self.nonce {
                nonce
            } else {
                let account = accounts::get(&client, &keypair.public_key().to_string()).await?;
                account.speculative_nonce + 1
            },
            signature: Vec::new(),
        };

        txn.fee = if let Some(fee) = self.fee {
            fee
        } else {
            txn.txn_fee(&get_txn_fees(&client).await?)?
        };
        txn.signature = txn.sign(&keypair)?;

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
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
            table.add_row(row!["Payee", "Amount", "Memo"]);
            for payment in txn.payments.clone() {
                table.add_row(row![
                    PublicKey::from_bytes(payment.payee)?.to_string(),
                    Hnt::from(payment.amount),
                    Memo::from(payment.memo).to_string(),
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
                    "payee": PublicKey::from_bytes(payment.payee)?.to_string(),
                    "amount": Hnt::from(payment.amount),
                    "memo": Memo::from(payment.memo).to_string()
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
    address: PublicKey,
    amount: Hnt,
    memo: Memo,
}

use crate::result::{anyhow, bail};

impl FromStr for Payee {
    type Err = crate::result::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // String is of the format:
        //    <address>?amount=<amount>?memo=<memo>?memo_type=<string|binary>
        let mut split = s.split('?');

        // First segment is always address
        if let Some(address) = split.next() {
            // Memo defaults to 0
            let mut memo = Memo::default();
            // Initialize amount as option, but we require amount later
            let mut amount = None;

            // Remaining segments are arguments of the form key=<value>
            for segment in split {
                let pos = segment
                    .find('=')
                    .ok_or_else(|| anyhow!("invalid KEY=value: missing `=`  in `{}`", segment))?;
                let key = &segment[..pos];
                let value = &segment[pos + 1..];
                match key {
                    "amount" => {
                        amount = Some(value.parse()?);
                    }
                    "memo" => {
                        memo = Memo::from_str(value)?;
                    }
                    _ => bail!("Invalid key given: {}", key),
                }
            }
            Ok(Payee {
                address: address.parse()?,
                amount: if let Some(amount) = amount {
                    amount
                } else {
                    bail!("Pay transaction must set amount")
                },
                memo,
            })
        } else {
            bail!("Invalid command syntax. Check --help for more information")
        }
    }
}
