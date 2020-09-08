use crate::{
    cmd::{
        api_url, load_wallet, print_table, Opts,
    },
    result::Result,
};
use helium_api::{Client};
use prettytable::{Cell, Row, Table};
use structopt::StructOpt;
use std::fs::File;
use chrono::{DateTime, NaiveDateTime, Utc};

#[derive(Debug, StructOpt)]
/// Print recent transactions and pending
pub struct Cmd {
    /// optionally input an address instead of using file
    #[structopt(long, short)]
    address: Option<String>,

    /// fetch all transactions instead of just recent
    #[structopt(long)]
    all: bool,

    /// output csv
    #[structopt(long)]
    csv: bool
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {

        let address = if let Some(address) = &self.address {
            String::from(address)
        } else {
            load_wallet(opts.files)?.address()?
        };

        let client = Client::new_with_base_url(api_url());

        let (transactions, mut cursor) = client.get_account_transactions(&address)?;

        if self.all {
            println!("Fetching all transactions for {}", address);
        } else {
            println!("Fetching recent transactions for {}", address);
        }

        let mut table = Table::new();
        table.add_row(row!["Type", "Date", "Block", "Hash", "Counterparty", "Bones"]);
        if let Some(transactions) = transactions {
            for txn in transactions {
                table.add_row(txn.into_row(&address));
            }
        }

        if self.all {
            let mut errors = 0;
            while let Some(actual_cursor) = &cursor {
                match client.get_more_account_transactions(&address, &actual_cursor) {
                    Ok((transactions, new_cursor)) => {
                        if let Some(transactions) = transactions {
                            for txn in transactions {
                                table.add_row(txn.into_row(&address));
                            }                        }
                        errors = 0;
                        cursor = new_cursor;
                    }
                    Err(e) => {
                        // if this has happened less than 3 times,
                        // back off the API and wait
                        if errors <= 3 {
                            println!("Error as occured");
                            use std::{thread, time};
                            errors+=1;
                            thread::sleep(time::Duration::from_secs(1));
                        }
                        // if this has happend 3 times in a row, give up
                        else {
                            panic!("Error fetching account transactions: {}", e)

                        }
                    }
                }
            }
        }
        print_table(&table)?;

        if self.csv {
            let time: DateTime<Utc> = Utc::now();
            let out = File::create(format!("{}_{}.csv", address, time.format("%Y-%m-%d_%H-%M-%S").to_string()))?;
            table.to_csv(out)?;
        }


        Ok(())
    }
}

use helium_api::transactions::{Reward, Transaction};

trait Rowify {
    fn into_row(&self, account: &str) -> Row;
}

impl Rowify for Transaction {
    fn into_row(&self, account: &str) -> Row {
        match self {
            Transaction::PaymentV1(payment) => {
                // This account is paying HNT
                let(counterparty, amount) = if account == payment.payer {
                    (Cell::new(payment.payee.as_str()), Cell::new(format!("-{}", payment.amount).as_str()))
                }
                // this account is receiving HNT
                else {
                    (Cell::new(payment.payer.as_str()), Cell::new(format!("{: >21}", payment.amount).as_str()))
                };

                let timestamp =  DateTime::<Utc>::from_utc(
                    NaiveDateTime::from_timestamp(payment.time as i64,0), Utc
                );

                Row::new(vec![
                    Cell::new(format!("{: <25}", "PaymentV1").as_str()),
                    Cell::new(&timestamp.to_rfc3339()),
                    Cell::new(format!("{}", payment.height).as_str()),
                    Cell::new(payment.hash.as_str()),
                    counterparty,
                    amount,
                ])
            },
            Transaction::PaymentV2(payment_v2) => {
                let timestamp =  DateTime::<Utc>::from_utc(
                    NaiveDateTime::from_timestamp(payment_v2.time as i64,0), Utc
                );
                // This account is paying HNT
                let(counterparty, amount) = if account == payment_v2.payer {

                    let counterparty = if payment_v2.payments.len() == 1 {
                        Cell::new(payment_v2.payments[0].payee.as_str())

                    } else {
                        Cell::new(format!("{: <52}", "many_payees").as_str())
                    };

                    let amount = {
                        let mut total = 0;
                        for payment in &payment_v2.payments {
                            total += payment.amount;
                        }
                        total
                    };
                    (counterparty, Cell::new(format!("-{}", amount).as_str()))
                }
                // this account is receiving HNT
                else {
                    {
                        let mut amount = 0;

                        for payment in &payment_v2.payments {
                            if account == payment.payee {
                                amount += payment.amount;
                            }
                        }
                        (Cell::new(payment_v2.payer.as_str()), Cell::new(format!("{: >21}", amount).as_str()))
                    }
                };

                Row::new(vec![
                    Cell::new(format!("{: <25}", "PaymentV2").as_str()),
                    Cell::new(&timestamp.to_rfc3339()),
                    Cell::new(format!("{}", payment_v2.height).as_str()),
                    Cell::new(payment_v2.hash.as_str()),
                    counterparty,
                    amount
                ])
            },
            Transaction::RewardsV1(reward) => {
                let timestamp =  DateTime::<Utc>::from_utc(
                    NaiveDateTime::from_timestamp(reward.time as i64,0), Utc
                );
                let mut total = 0;

                // summate rewards for all reward types
                for reward in &reward.rewards {
                    total += match reward {
                        Reward::Securities(data) => data.amount,
                        Reward::DataCredits(data) => data.amount,
                        Reward::PocChallengees(data) => data.amount,
                        Reward::PocChallengers(data) => data.amount,
                        Reward::PocWitnesses(data) => data.amount,
                        Reward::Consensus(data) =>  data.amount,
                    };
                }
                Row::new(vec![
                    Cell::new(format!("{: <25}", "RewardsV1").as_str()),
                    Cell::new(&timestamp.to_rfc3339()),
                    Cell::new(format!("{}", reward.height).as_str()),
                    Cell::new(reward.hash.as_str()),
                    Cell::new(format!("{: <52}", "rewards").as_str()),
                    Cell::new(format!("{}", total).as_str()),
                ])
            },
            Transaction::TokenBurnV1(burn) => {
                let timestamp =  DateTime::<Utc>::from_utc(
                    NaiveDateTime::from_timestamp(burn.time as i64,0), Utc
                );

                // This account is burning HNT
                let amount = if account == burn.payer {
                    Cell::new(format!("-{}", burn.amount).as_str())
                }
                // This account is not burning any HNT,
                // so it must just be receiving the DC
                else {
                    Cell::new(format!("{}", 0).as_str())
                };

                Row::new(vec![
                    Cell::new(format!("{: <25}", "TokenBurnV1").as_str()),
                    Cell::new(&timestamp.to_rfc3339()),
                    Cell::new(format!("{}", burn.height).as_str()),
                    Cell::new(burn.hash.as_str()),
                    Cell::new(burn.payee.as_str()),
                    amount,
                ])
            },
            Transaction::AddGatewayV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "AddGatewayV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::AssertLocationV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "AssertLocationV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::CoinbaseV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "CoinbaseV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::CreateHtlcV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "CreateHtlcV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::GenGatewayV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "GenGatewayV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::ConsensusGroupV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "ConsensusGroupV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::OuiV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "OuiV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::PocReceiptsV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "PocReceiptsV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::PocRequestV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "PocRequestV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::RedeemHtlcV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "RedeemHtlcV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::SecurityCoinbaseV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "SecurityCoinbaseV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::RoutingV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "RoutingV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::SecurityExchangeV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "SecurityExchangeV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::VarsV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "VarsV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::DcCoinbaseV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "DcCoinbaseV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::TokenBurnExchangeRateV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "TokenBurnExchangeRateV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::BundleV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "BundleV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::StateChannelOpenV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "StateChannelOpenV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::UpdateGatewayOuiV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "UpdateGatewayOuiV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::StateChannelCloseV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "StateChannelCloseV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::PriceOracleV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "PriceOracleV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
            Transaction::GenPriceOracleV1(_) => Row::new(vec![
                Cell::new(format!("{: <25}", "GenPriceOracleV1").as_str()),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
                Cell::new(""),
            ]),
        }
    }
}
