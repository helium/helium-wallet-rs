use crate::{
    cmd::{api_url, load_wallet, print_table, Opts},
    result::Result,
};
use chrono::{DateTime, Utc};
use helium_api::Client;
use prettytable::Table;
use std::fs::File;
use structopt::StructOpt;

mod accounting;
use accounting::IntoRow;

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
    csv: bool,
}

pub struct Balance {
    bones: u64,
    dc: u64,
}

struct Difference {
    counterparty: Option<String>,
    bones: isize,
    dc: isize,
}

impl Balance {
    fn update(&mut self, diff: &Difference) {
        self.bones = (self.bones as isize + diff.bones) as u64;
        self.dc = (self.dc as isize + diff.dc) as u64;
    }
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

        let mut all_transactions = Vec::new();

        if let Some(transactions) = transactions {
            all_transactions.extend(transactions);
        }

        if self.all {
            let mut errors = 0;
            while let Some(actual_cursor) = &cursor {
                match client.get_more_account_transactions(&address, &actual_cursor) {
                    Ok((transactions, new_cursor)) => {
                        if let Some(transactions) = transactions {
                            all_transactions.extend(transactions);
                        }
                        errors = 0;
                        cursor = new_cursor;
                    }
                    Err(e) => {
                        // if this has happened less than 3 times,
                        // back off the API and wait
                        if errors <= 3 {
                            println!("Error has occurred");
                            use std::{thread, time};
                            errors += 1;
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

        all_transactions.reverse();

        let mut table = Table::new();
        table.add_row(row![
            "Type",
            "Date",
            "Block",
            "Hash",
            "Counterparty",
            "+/- Bones",
            "New Balance B",
            "+/- DC",
            "New Balance DC",
        ]);

        let mut balance = Balance { bones: 0, dc: 0 };

        for transaction in all_transactions {
            table.add_row(transaction.into_row(&address, &mut balance, &client));
        }

        print_table(&table)?;

        if self.csv {
            let time: DateTime<Utc> = Utc::now();
            let out = File::create(format!(
                "{}_{}.csv",
                address,
                time.format("%Y-%m-%d_%H-%M-%S").to_string()
            ))?;
            table.to_csv(out)?;
        }

        Ok(())
    }
}
