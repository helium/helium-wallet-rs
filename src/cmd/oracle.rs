use crate::{
    cmd::*,
    result::{anyhow, Result},
    traits::{TxnEnvelope, TxnSign, B64},
};
use helium_api::blocks;
use rust_decimal::{prelude::*, Decimal};
use serde::Serialize;
use serde_json::json;
use std::str::FromStr;

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
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Report(cmd) => cmd.run(opts).await,
        }
    }
}

impl Report {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let client = new_client(api_url(wallet.public_key.network));
        let block_height = self.block.to_block(&client).await?;
        let price = u64::from(self.price.to_usd().await?);
        let mut txn = BlockchainTxnPriceOracleV1 {
            public_key: keypair.public_key().into(),
            price,
            block_height,
            signature: Vec::new(),
        };
        txn.signature = txn.sign(&keypair)?;

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
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
                ["Price", Usd::from(txn.price)],
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
enum Block {
    Auto,
    Height(u64),
}

impl FromStr for Block {
    type Err = Box<dyn std::error::Error>;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Block::Auto),
            _ => Ok(Block::Height(s.parse()?)),
        }
    }
}

impl Block {
    async fn to_block(self, client: &Client) -> Result<u64> {
        match self {
            Block::Auto => Ok(blocks::height(client).await?),
            Block::Height(height) => Ok(height),
        }
    }
}

#[derive(Debug)]
enum Price {
    CoinGecko,
    Bilaxy,
    BinanceUs,
    BinanceInt,
    Ftx,
    Usd(Usd),
}

impl Price {
    async fn to_usd(&self) -> Result<Usd> {
        match self {
            Self::CoinGecko => {
                let response =
                    reqwest::get("https://api.coingecko.com/api/v3/coins/helium").await?;
                let json: serde_json::Value = response.json().await?;
                let amount = &json["market_data"]["current_price"]["usd"].to_string();
                Ok(Usd::from_str(amount)?)
            }
            Self::Bilaxy => {
                let response =
                    reqwest::get("https://newapi.bilaxy.com/v1/valuation?currency=HNT").await?;
                let json: serde_json::Value = response.json().await?;
                let amount = &json["HNT"]["usd_value"]
                    .as_str()
                    .ok_or_else(|| anyhow!("No USD value found"))?;
                Ok(Usd::from_str(amount)?)
            }
            Self::BinanceUs => {
                let response =
                    reqwest::get("https://api.binance.us/api/v3/ticker/price?symbol=HNTUSD")
                        .await?;
                let json: serde_json::Value = response.json().await?;
                let amount = &json["price"]
                    .as_str()
                    .ok_or_else(|| anyhow!("No USD value found"))?;
                Ok(Usd::from_str(amount)?)
            }
            Self::BinanceInt => {
                let response =
                    reqwest::get("https://api.binance.us/api/v3/avgPrice?symbol=HNTUSDT").await?;
                let json: serde_json::Value = response.json().await?;
                let amount = &json["price"]
                    .as_str()
                    .ok_or_else(|| anyhow!("No USD value found"))?;
                Ok(Usd::from_str(amount)?)
            }
            Self::Ftx => {
                let response = reqwest::get("https://ftx.com/api/markets/HNT/USD").await?;
                let json: serde_json::Value = response.json().await?;
                let amount = &json["result"]["price"].to_string();
                Ok(Usd::from_str(amount)?)
            }
            Self::Usd(v) => Ok(*v),
        }
    }
}

impl FromStr for Price {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "coingecko" => Ok(Self::CoinGecko),
            "bilaxy" => Ok(Self::Bilaxy),
            // don't break old interface so maintain "binance" to Binance US
            "binance" => Ok(Self::BinanceUs),
            "binance-us" => Ok(Self::BinanceUs),
            "binance-int" => Ok(Self::BinanceInt),
            "ftx" => Ok(Self::Ftx),
            _ => {
                let data = Decimal::from_str(s).or_else(|_| Decimal::from_scientific(s))?;
                Ok(Self::Usd(Usd::new(data.round_dp_with_strategy(
                    8,
                    RoundingStrategy::MidpointAwayFromZero,
                ))))
            }
        }
    }
}
