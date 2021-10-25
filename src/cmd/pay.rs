use crate::{
    cmd::*,
    keypair::PublicKey,
    memo::Memo,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, B64},
};
use helium_api::accounts;
use prettytable::Table;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, StructOpt)]
/// Send one (or more) payments to given addresses.
///
/// The payment is not submitted to the system unless the '--commit' option is
/// given.
pub enum Cmd {
    /// Pay a single payee.
    ///
    /// Note that HNT only goes to 8 decimals of precision.
    One(Box<One>),
    /// Pay multiple payees
    Multi(Multi),
}

#[derive(Debug, StructOpt)]
pub struct One {
    #[structopt(flatten)]
    payee: Payee,
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

#[derive(Debug, StructOpt)]
/// The input file for multiple payments is expected to be json file with a list
/// of payees, amounts, and optional memos. For example:
///
/// [
///     {
///         "address": "<adddress1>",
///         "amount": 1.6,
///         "memo": "AAAAAAAAAAA="
///     },
///     {
///         "address": "<adddress2>",
///         "amount": 0.5
///     }
/// ]
///
/// Note that HNT only goes to 8 decimals of precision.
pub struct Multi {
    /// File to read multiple payments from.
    path: PathBuf,
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
        let payments = self.collect_payments()?;

        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;

        let client = Client::new_with_base_url(api_url(wallet.public_key.network));

        let keypair = wallet.decrypt(password.as_bytes())?;

        let mut txn = BlockchainTxnPaymentV2 {
            fee: 0,
            payments,
            payer: keypair.public_key().to_vec(),
            nonce: if let Some(nonce) = self.nonce() {
                nonce
            } else {
                let account = accounts::get(&client, &keypair.public_key().to_string()).await?;
                account.speculative_nonce + 1
            },
            signature: Vec::new(),
        };

        txn.fee = if let Some(fee) = self.fee() {
            fee
        } else {
            txn.txn_fee(&get_txn_fees(&client).await?)?
        };
        txn.signature = txn.sign(&keypair)?;

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit(), &client, &envelope).await?;
        print_txn(&txn, &envelope, &status, opts.format)
    }

    fn collect_payments(&self) -> Result<Vec<Payment>> {
        match &self {
            Self::One(one) => Ok(vec![Payment {
                payee: one.payee.address.to_vec(),
                amount: u64::from(one.payee.amount),
                memo: u64::from(&one.payee.memo),
            }]),
            Self::Multi(multi) => {
                let file = std::fs::File::open(multi.path.clone())?;
                let payees: Vec<Payee> = serde_json::from_reader(file)?;
                let payments = payees
                    .iter()
                    .map(|p| Payment {
                        payee: p.address.to_vec(),
                        amount: u64::from(p.amount),
                        memo: u64::from(&p.memo),
                    })
                    .collect();
                Ok(payments)
            }
        }
    }

    fn nonce(&self) -> Option<u64> {
        match &self {
            Self::One(one) => one.nonce,
            Self::Multi(multi) => multi.nonce,
        }
    }

    fn fee(&self) -> Option<u64> {
        match &self {
            Self::One(one) => one.fee,
            Self::Multi(multi) => multi.fee,
        }
    }

    fn commit(&self) -> bool {
        match &self {
            Self::One(one) => one.commit,
            Self::Multi(multi) => multi.commit,
        }
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
            table.add_row(row!["Payee", "Amount (HNT)", "Memo"]);
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
                ["Fee (DC)", txn.fee],
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

#[derive(Debug, Deserialize, StructOpt)]
pub struct Payee {
    /// Address to send the tokens to.
    address: PublicKey,
    /// Amount of HNT to send
    amount: Hnt,
    /// Memo field to include. Provide as a base64 encoded string
    #[serde(default)]
    #[structopt(long, default_value = "AAAAAAAAAAA=")]
    memo: Memo,
}
