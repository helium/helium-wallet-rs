use crate::{
    cmd::*,
    keypair::PublicKey,
    memo::Memo,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, B64},
};
use helium_api::accounts;
use helium_proto::BlockchainTokenTypeV1;
use prettytable::Table;
use serde::Deserialize;
use serde_json::json;
use std::str::FromStr;

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
/// of payees, amounts token, and optional memos.
///
/// For example:
///
/// [
///     {
///         "address": "<adddress1>",
///         "amount": 1.6,
///         "memo": "AAAAAAAAAAA=",
///         "token": "Hnt",
///     },
///     {
///         "address": "<adddress2>",
///         "amount": 0.5,
///         "token": "Mobile"
///     }
/// ]
///
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
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let base_url: String = api_url(wallet.public_key.network);
        let client = new_client(base_url.clone());
        let pending_url = base_url + "/pending_transactions/";
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
        print_txn(&txn, &envelope, &status, &pending_url, opts.format)
    }

    fn collect_payments(&self) -> Result<Vec<Payment>> {
        match &self {
            Self::One(one) => Ok(vec![Payment {
                payee: one.payee.address.to_vec(),
                // we safely create u64 from the amount of type Token
                // only because each token_type has the same amount of decimals
                amount: u64::from(one.payee.amount),
                memo: u64::from(&one.payee.memo),
                max: false,
                token_type: match one.payee.token {
                    TokenInput::Hnt => BlockchainTokenTypeV1::Hnt.into(),
                    TokenInput::Hst => BlockchainTokenTypeV1::Hst.into(),
                    TokenInput::Iot => BlockchainTokenTypeV1::Iot.into(),
                    TokenInput::Mobile => BlockchainTokenTypeV1::Mobile.into(),
                },
            }]),
            Self::Multi(multi) => {
                let file = std::fs::File::open(multi.path.clone())?;
                let payees: Vec<Payee> = serde_json::from_reader(file)?;
                let payments = payees
                    .iter()
                    .map(|p| Payment {
                        payee: p.address.to_vec(),
                        // we safely create u64 from the amount of type Token
                        // only because each token_type has the same amount of decimals
                        amount: u64::from(p.amount),
                        memo: u64::from(&p.memo),
                        max: false,
                        token_type: match p.token {
                            TokenInput::Hnt => BlockchainTokenTypeV1::Hnt.into(),
                            TokenInput::Hst => BlockchainTokenTypeV1::Hst.into(),
                            TokenInput::Iot => BlockchainTokenTypeV1::Iot.into(),
                            TokenInput::Mobile => BlockchainTokenTypeV1::Mobile.into(),
                        },
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
    pending_url: &str,
    format: OutputFormat,
) -> Result {
    let status_endpoint = pending_url.to_owned() + status_str(status);
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();

            table.add_row(row!["Payee", "Amount", "Memo"]);
            for payment in txn.payments.clone() {
                let token_type = BlockchainTokenTypeV1::from_i32(payment.token_type)
                    .expect("Invalid token_type found in transaction!");
                let amount_decimal = Token::from(payment.amount);
                let amount_units = match token_type {
                    BlockchainTokenTypeV1::Hnt => "HNT",
                    BlockchainTokenTypeV1::Hst => "HST",
                    BlockchainTokenTypeV1::Iot => "IOT",
                    BlockchainTokenTypeV1::Mobile => "MOBILE",
                };

                table.add_row(row![
                    PublicKey::from_bytes(payment.payee)?.to_string(),
                    format!("{amount_decimal} {amount_units}"),
                    Memo::from(payment.memo).to_string(),
                ]);
            }
            print_table(&table, None)?;

            ptable!(
                ["Key", "Value"],
                ["Fee (DC)", txn.fee],
                ["Nonce", txn.nonce],
                ["Hash", status_str(status)],
                ["Status", status_endpoint]
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
                "status": status_endpoint
            });
            print_json(&table)
        }
    }
}

#[derive(Debug, Deserialize, StructOpt)]
pub struct Payee {
    /// Address to send the tokens to.
    address: PublicKey,
    /// Amount of token to send
    #[serde(deserialize_with = "token_decimal_deserializer")]
    amount: Token,
    /// Type of token to send (hnt, iot, mobile, hst).
    #[serde(default)]
    #[structopt(default_value = "hnt")]
    token: TokenInput,
    /// Memo field to include. Provide as a base64 encoded string
    #[serde(default)]
    #[structopt(long, default_value = "AAAAAAAAAAA=")]
    memo: Memo,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
pub enum TokenInput {
    Hnt,
    Iot,
    Mobile,
    Hst,
}

// By default, helium-api-rs serializes and deserializes tokens as u64s (ie: bones).
// This overrides the default serializer when parsing JSON.
// Structopt deserializes using "from_str", which also expects Decimal representation
fn token_decimal_deserializer<'de, D>(deserializer: D) -> std::result::Result<Token, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let decimal: rust_decimal::Decimal = Deserialize::deserialize(deserializer)?;
    Ok(Token::new(decimal))
}

impl FromStr for TokenInput {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.to_lowercase();
        match s.as_str() {
            "hnt" => Ok(TokenInput::Hnt),
            "iot" => Ok(TokenInput::Iot),
            "mob" | "mobile" => Ok(TokenInput::Mobile),
            "hst" => Ok(TokenInput::Hst),
            _ => Err(anyhow::anyhow!("Invalid token input {s}")),
        }
    }
}

impl Default for TokenInput {
    fn default() -> Self {
        Self::Hnt
    }
}

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_json_hnt_input() {
        let json_hnt_input = "{\
            \"address\": \"13buBykFQf5VaQtv7mWj2PBY9Lq4i1DeXhg7C4Vbu3ppzqqNkTH\",\
            \"amount\": 1.6,\
            \"memo\": \"AAAAAAAAAAA=\"\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).unwrap();
        assert_eq!(Token::from(160000000), payee.amount);
    }

    #[test]
    fn test_json_mobile_input() {
        let json_hnt_input = "{\
            \"address\": \"13buBykFQf5VaQtv7mWj2PBY9Lq4i1DeXhg7C4Vbu3ppzqqNkTH\",\
            \"amount\": 0.5,\
            \"token\": \"Mobile\"\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).unwrap();
        assert_eq!(Token::from(50000000), payee.amount);
        assert_eq!(TokenInput::Mobile, payee.token);
    }
}
