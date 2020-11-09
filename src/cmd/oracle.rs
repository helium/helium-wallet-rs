use crate::{
    cmd::{
        api_url, get_password, load_wallet, print_footer, print_json, status_json, status_str,
        Opts, OutputFormat,
    },
    result::Result,
    traits::{Sign, TxnEnvelope, B64},
};
use helium_api::{BlockchainTxn, BlockchainTxnPriceOracleV1, Client, PendingTxnStatus};
use rust_decimal::{prelude::*, Decimal};
use serde::Serialize;
use serde_json::json;
use std::str::FromStr;
use structopt::StructOpt;

/// Report an oracle price to the blockchain
#[derive(Debug, StructOpt)]
pub enum Cmd {
    Report(Report),
}

#[derive(Debug, StructOpt)]
/// Construct an oracle price report and optionally commit it to the
/// Helium Blockchain.
pub struct Report {
    /// The oracle price to report. Specify in USD or supply one of the
    /// supported price lookup services ("coingecko", "bilaxy", "binance").
    #[structopt(long)]
    price: Price,

    /// Block height to report the price at. Use "auto" to pick the
    /// latest known block height from the API.
    #[structopt(long)]
    block: Block,

    /// Commit the oracle price report to the API
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Report(cmd) => cmd.run(opts),
        }
    }
}

impl Report {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let client = Client::new_with_base_url(api_url());

        let mut txn = BlockchainTxnPriceOracleV1 {
            public_key: keypair.pubkey_bin().into(),
            price: self.price.to_millis(),
            block_height: self.block.to_block(),
            signature: Vec::new(),
        };
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
    txn: &BlockchainTxnPriceOracleV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let encoded = envelope.to_b64()?;
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Block Height", txn.block_height],
                ["Price", Price::from_millis(txn.price)],
                ["Hash", status_str(status)]
            );

            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "price": txn.price,
                "block_height": txn.block_height,
                "txn": encoded,
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Block(u64);

impl FromStr for Block {
    type Err = Box<dyn std::error::Error>;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "auto" => {
                let client = Client::new_with_base_url(api_url());
                Ok(Block(client.get_height()?))
            }
            _ => Ok(Block(s.parse()?)),
        }
    }
}

impl Block {
    fn to_block(self) -> u64 {
        self.0
    }
}

const USD_TO_PRICE_SCALAR: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, Serialize)]
struct Price(Decimal);

impl Price {
    fn from_coingecko() -> Result<Self> {
        let mut response = reqwest::get("https://api.coingecko.com/api/v3/coins/helium")?;
        let json: serde_json::Value = response.json()?;
        let amount = &json["market_data"]["current_price"]["usd"];
        Price::from_str(&amount.to_string())
    }

    fn from_bilaxy() -> Result<Self> {
        let mut response = reqwest::get("https://newapi.bilaxy.com/v1/valuation?currency=HNT")?;
        let json: serde_json::Value = response.json()?;
        let amount = &json["HNT"]["usd_value"];
        Price::from_str(amount.as_str().ok_or("No USD value found")?)
    }

    fn from_binance() -> Result<Self> {
        let mut response =
            reqwest::get("https://api.binance.us/api/v3/ticker/price?symbol=HNTUSD")?;
        let json: serde_json::Value = response.json()?;
        let amount = &json["price"];
        Price::from_str(amount.as_str().ok_or("No USD value found")?)
    }

    fn to_millis(self) -> u64 {
        if let Some(scaled_dec) = self.0.checked_mul(USD_TO_PRICE_SCALAR.into()) {
            if let Some(num) = scaled_dec.to_u64() {
                return num;
            }
        }
        panic!("Price has been constructed with invalid data")
    }

    fn from_millis(millis: u64) -> Self {
        if let Some(mut data) = Decimal::from_u64(millis) {
            data.set_scale(8).unwrap();
            return Price(data);
        }
        panic!("Price value could not be parsed into Decimal")
    }
}

impl FromStr for Price {
    type Err = Box<dyn std::error::Error>;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "coingecko" => Price::from_coingecko(),
            "bilaxy" => Price::from_bilaxy(),
            "binance" => Price::from_binance(),
            _ => {
                let data = Decimal::from_str(s).or_else(|_| Decimal::from_scientific(s))?;
                Ok(Price(
                    data.round_dp_with_strategy(8, RoundingStrategy::RoundHalfUp),
                ))
            }
        }
    }
}

impl ToString for Price {
    fn to_string(&self) -> String {
        self.0.to_string()
    }
}
