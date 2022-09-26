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
use rust_decimal::Decimal;
use serde::{de::Error as deError, Deserialize, Deserializer};
use serde_json::{json, Value};
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
/// of payees, amounts, tokens, and optional memos.
/// Notes:
///   "address" is required.
///   "amount" is required. It must be a number or the string "max". When "max"
///            the entire balance (minus fees) will be sent.
///   "token" is optional and defaults to "Hnt".
///   "memo" is optional.
///
/// For example:
///
/// [
///     {
///         "address": "<address1>",
///         "amount": 1.6,
///         "memo": "AAAAAAAAAAA=",
///         "token": "Hnt"
///     },
///     {
///         "address": "<address2>",
///         "amount": "max"
///     },
///     {
///         "address": "<address3>",
///         "amount": 3,
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
            Self::One(one) => {
                Ok(vec![Payment {
                    payee: one.payee.address.to_vec(),
                    // we safely create u64 from the amount of type Token
                    // only because each token_type has the same amount of decimals
                    amount: u64::from(one.payee.amount.token_amount()),
                    memo: u64::from(&one.payee.memo),
                    max: one.payee.amount == Amount::Max,
                    token_type: match one.payee.token {
                        TokenInput::Hnt => BlockchainTokenTypeV1::Hnt.into(),
                        TokenInput::Hst => BlockchainTokenTypeV1::Hst.into(),
                        TokenInput::Iot => BlockchainTokenTypeV1::Iot.into(),
                        TokenInput::Mobile => BlockchainTokenTypeV1::Mobile.into(),
                    },
                }])
            }
            Self::Multi(multi) => {
                let file = std::fs::File::open(multi.path.clone())?;
                let payees: Vec<Payee> = serde_json::from_reader(file)?;
                let payments = payees
                    .iter()
                    .map(|p| Payment {
                        payee: p.address.to_vec(),
                        // we safely create u64 from the amount of type Token
                        // only because each token_type has the same amount of decimals
                        amount: u64::from(p.amount.token_amount()),
                        memo: u64::from(&p.memo),
                        max: p.amount == Amount::Max,
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
                let amount_str = if payment.max {
                    String::from("**MAX**")
                } else {
                    Token::from(payment.amount).to_string()
                };
                let amount_units = BlockchainTokenTypeV1::from_i32(payment.token_type)
                    .expect("Invalid token_type found in transaction!")
                    .as_str_name();

                table.add_row(row![
                    PublicKey::from_bytes(payment.payee)?.to_string(),
                    format!("{amount_str} {amount_units}"),
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
                let token_type = BlockchainTokenTypeV1::from_i32(payment.token_type)
                    .expect("Invalid token_type found in transaction!");
                payments.push(json!({
                    "payee": PublicKey::from_bytes(payment.payee)?.to_string(),
                    "amount": Token::from(payment.amount).to_f64(),
                    "token_type": token_type.as_str_name(),
                    "max": payment.max,
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
    amount: Amount,
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

#[derive(Debug, PartialEq)]
// An amount can be either the string "max" or a number, the deserializer handles both cases.
// token_amount() will return 0 (when Amount::Max) or a valid token amount. Use this instead
// of Token::Dec directly.
//
// We use decimal for all numbers because the helium-api-rs serializes and deserializes tokens as
// u64s (ie: bones).
enum Amount {
    Token(Token),
    Max,
}

impl Amount {
    pub fn token_amount(&self) -> Token {
        match self {
            Amount::Token(raw) => *raw,
            _ => Token::new(Decimal::from(0)),
        }
    }
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Amount, D::Error>
    where
        D: Deserializer<'de>,
    {
        // use the JSON deserialize so as to accept strings or nums
        let s = Value::deserialize(deserializer)?.to_string();
        Amount::from_str(&s).map_err(D::Error::custom)
    }
}

impl FromStr for Amount {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        if let Ok(t) = Token::from_str(s) {
            Ok(Amount::Token(t))
        } else if s.eq("max") || s.eq("\"max\"") {
            Ok(Amount::Max)
        } else {
            Err(anyhow::anyhow!(
                "Invalid amount \"{}\" Amount must be a number or \"max\"",
                s
            ))
        }
    }
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
        assert_eq!(Token::from(160000000), payee.amount.token_amount());
        assert_ne!(payee.amount, Amount::Max);
    }

    #[test]
    fn test_json_mobile_input() {
        let json_hnt_input = "{\
            \"address\": \"13buBykFQf5VaQtv7mWj2PBY9Lq4i1DeXhg7C4Vbu3ppzqqNkTH\",\
            \"amount\": 0.5,\
            \"token\": \"Mobile\"\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).unwrap();
        assert_eq!(Token::from(50000000), payee.amount.token_amount());
        assert_ne!(payee.amount, Amount::Max);
        assert_eq!(TokenInput::Mobile, payee.token);
    }

    #[test]
    fn test_json_max_input() {
        let json_hnt_input = "{\
            \"address\": \"13buBykFQf5VaQtv7mWj2PBY9Lq4i1DeXhg7C4Vbu3ppzqqNkTH\",\
            \"amount\": \"max\"\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).unwrap();
        assert_eq!(Token::from(0), payee.amount.token_amount());
        assert_eq!(payee.amount, Amount::Max);
    }

    #[test]
    fn test_json_bad_amount() {
        let json_hnt_input = "{\
            \"address\": \"13buBykFQf5VaQtv7mWj2PBY9Lq4i1DeXhg7C4Vbu3ppzqqNkTH\",\
            \"amount\": \"foo\",\
        }";

        let result: std::result::Result<Payee, serde_json::Error> =
            serde_json::from_str(json_hnt_input);
        assert!(result.is_err());
    }

    #[test]
    fn test_json_defaults_input() {
        let json_hnt_input = "{\
            \"address\": \"13buBykFQf5VaQtv7mWj2PBY9Lq4i1DeXhg7C4Vbu3ppzqqNkTH\",\
            \"amount\": 12\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).unwrap();
        assert_eq!(Token::from(1200000000), payee.amount.token_amount());
        assert_ne!(payee.amount, Amount::Max);
        assert_eq!(TokenInput::Hnt, payee.token);
        assert_eq!(Memo::default(), payee.memo);
    }
}
