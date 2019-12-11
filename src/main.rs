#[macro_use]
extern crate prettytable;
#[macro_use]
extern crate lazy_static;

mod cmd_balance;
mod cmd_create;
mod cmd_hotspots;
mod cmd_info;
mod cmd_pay;
mod cmd_verify;
mod keypair;
mod mnemonic;
mod result;
mod traits;
mod wallet;
mod ledger_api;

use crate::{result::Result, traits::ReadWrite, wallet::Wallet};
use std::path::PathBuf;
use std::{fs, process};
use structopt::StructOpt;

/// Create and manage Helium wallets
#[derive(Debug, StructOpt)]
enum Cli {
    /// Get wallet information
    Info {
        /// Whether to use Ledger
        #[structopt(short = "l", long = "ledger")]
        ledger: bool,

        /// File(s) to print information on
        #[structopt(short = "f", long = "file", default_value = "wallet.key")]
        files: Vec<PathBuf>,

        /// Display QR code for a given single wallet.
        #[structopt(long = "qr")]
        qr_code: bool,
    },
    /// Verify an encypted wallet
    Verify {
        /// File(s) to verify
        #[structopt(short = "f", long = "file", default_value = "wallet.key")]
        files: Vec<PathBuf>,
    },
    /// Create a new wallet
    Create(CreateCmd),
    /// Get the balance for a wallet. The balance is given in bones,
    /// which is the smallest denomination for HNT. 1 HNT is 100_000_000
    /// bones
    Balance {
        /// Wallet(s) to read addresses from
        #[structopt(short = "f", long = "file")]
        files: Vec<PathBuf>,

        /// Addresses to get balances for
        #[structopt(short = "a", long = "address")]
        addresses: Vec<String>,
    },
    /// Get the hotspots for a wallet
    Hotspots {
        /// Wallet(s) to read addresses from
        #[structopt(short = "f", long = "file")]
        files: Vec<PathBuf>,

        /// Addresses to get balances for
        #[structopt(short = "a", long = "address")]
        addresses: Vec<String>,
    },
    /// Pay a number of bones to a given address. Note that 1 HNT is
    /// 100_000_000 bones
    Pay {
        /// Whether to use Ledger
        #[structopt(short = "l", long = "ledger")]
        ledger: bool,

        /// Wallet to use as the payer
        #[structopt(short = "f", long = "file", default_value = "wallet.key")]
        files: Vec<PathBuf>,

        /// Address of the payee
        address: String,

        /// Number of bones to send
        #[structopt(name = "bones")]
        amount: u64,
    },
}

#[derive(Debug, StructOpt)]
/// Create a new wallet
pub enum CreateCmd {
    /// Create a new basic wallet
    Basic {
        #[structopt(short, long, default_value = "wallet.key")]
        /// Output file to store the key in
        output: PathBuf,

        #[structopt(long)]
        /// Overwrite an existing file
        force: bool,

        #[structopt(short = "i", long = "iterations", default_value = "1000000")]
        /// Number of PBKDF2 interations
        iterations: u32,

        #[structopt(long)]
        /// Use seed words to create the wallet
        seed: bool,
    },

    Sharded {
        #[structopt(short, long, default_value = "wallet.key")]
        /// Output file to store the key in
        output: PathBuf,

        #[structopt(long)]
        /// Overwrite an existing file
        force: bool,

        #[structopt(short = "i", long = "iterations", default_value = "1000000")]
        /// Number of PBKDF2 interations
        iterations: u32,

        #[structopt(short = "n", long = "shards", default_value = "5")]
        /// Number of shards to break the key into
        key_share_count: u8,

        #[structopt(short = "k", long = "required-shards", default_value = "3")]
        /// Number of shards required to recover the key
        recovery_threshold: u8,

        #[structopt(long)]
        /// Use seed words to create the wallet
        seed: bool,
    },
}

fn main() {
    let cli = Cli::from_args();
    if let Err(e) = run(cli) {
        println!("error: {}", e);
        process::exit(1);
    }
}

fn get_password(confirm: bool) -> std::io::Result<String> {
    use dialoguer::PasswordInput;
    let mut builder = PasswordInput::new();
    builder.with_prompt("Password");
    if confirm {
        builder.with_confirmation("Confirm password", "Passwords do not match");
    };
    builder.interact()
}

fn get_seed_words() -> Result<Vec<String>> {
    use dialoguer::Input;
    let split_str = |s: String| s.split_whitespace().map(|w| w.to_string()).collect();
    let word_string = Input::<String>::new()
        .with_prompt("Seed Words")
        .validate_with(move |v: &str| {
            let word_list = split_str(v.to_string());
            match mnemonic::mnemonic_to_entropy(word_list) {
                Ok(_) => Ok(()),
                Err(err) => Err(err),
            }
        })
        .interact()?;
    Ok(word_string
        .split_whitespace()
        .map(|w| w.to_string())
        .collect())
}



fn run(cli: Cli) -> Result {
    match cli {
        Cli::Info { ledger, files, qr_code } => {
            if ledger {
                ledger_api::load_wallet()
            } else {
                let wallet = load_wallet(files)?;
                cmd_info::cmd_info(&wallet, qr_code)
            }
        }
        Cli::Verify { files } => {
            let pass = get_password(false)?;
            let wallet = load_wallet(files)?;
            cmd_verify::cmd_verify(&wallet, &pass)
        }
        Cli::Create(CreateCmd::Basic {
            output,
            force,
            iterations,
            seed,
        }) => {
            let seed_words = if seed { Some(get_seed_words()?) } else { None };
            let pass = get_password(true)?;
            cmd_create::cmd_basic(&pass, iterations, output, force, seed_words)
        }
        Cli::Create(CreateCmd::Sharded {
            output,
            force,
            iterations,
            key_share_count,
            recovery_threshold,
            seed,
        }) => {
            let seed_words = if seed { Some(get_seed_words()?) } else { None };
            let pass = get_password(true)?;
            cmd_create::cmd_sharded(
                &pass,
                key_share_count,
                recovery_threshold,
                iterations,
                output,
                force,
                seed_words,
            )
        }
        Cli::Balance { files, addresses } => {
            cmd_balance::cmd_balance(collect_addresses(files, addresses)?)
        }
        Cli::Hotspots { files, addresses } => {
            cmd_hotspots::cmd_hotspots(collect_addresses(files, addresses)?)
        }
        Cli::Pay {
            ledger,
            address,
            amount,
            files,
        } => {
            if ledger {
                ledger_api::pay(address, amount)
            } else {
                let wallet = load_wallet(files)?;
                let pass = get_password(false)?;
                cmd_pay::cmd_pay(&wallet, &pass, address, amount)
            }
        }
    }
}

fn collect_addresses(files: Vec<PathBuf>, addresses: Vec<String>) -> Result<Vec<String>> {
    // If no files or addresses are given use the default wallet
    let file_list = if files.is_empty() && addresses.is_empty() {
        vec![PathBuf::from("wallet.key")]
    } else {
        files
    };
    let mut address_list = addresses.clone();
    for file in file_list {
        let mut reader = fs::File::open(&file)?;
        let enc_wallet = Wallet::read(&mut reader)?;
        address_list.push(enc_wallet.address()?);
    }
    Ok(address_list)
}

fn load_wallet(files: Vec<PathBuf>) -> Result<Wallet> {
    let mut files_iter = files.iter();
    let mut first_wallet = match files_iter.next() {
        Some(path) => {
            let mut reader = fs::File::open(path)?;
            Wallet::read(&mut reader)?
        }
        None => return Err("At least one wallet file expected".into()),
    };

    for path in files_iter {
        let mut reader = fs::File::open(path)?;
        let w = Wallet::read(&mut reader)?;
        let w_format = w.format.as_sharded_format()?;
        first_wallet.format.absorb_key_shares(&w_format)?;
    }

    Ok(first_wallet)
}
