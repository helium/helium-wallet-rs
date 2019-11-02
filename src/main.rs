#[macro_use]
extern crate prettytable;
mod cmd_balance;
mod cmd_create;
mod cmd_hotspots;
mod cmd_info;
mod cmd_verify;
mod keypair;
mod result;
mod traits;
mod wallet;

use crate::{
    traits::{ReadWrite, B58},
    wallet::Wallet,
    result::Result
};
use std::path::PathBuf;
use std::{fs, process};
use structopt::StructOpt;

/// Create and manage Helium wallets
#[derive(Debug, StructOpt)]
enum Cli {
    /// Get wallet information
    Info {
        #[structopt(short = "f", long = "file", default_value = "wallet.key")]
        files: Vec<PathBuf>,
    },
    /// Verify an encypted wallet
    Verify {
        /// File(s) to verify
        #[structopt(short = "f", long = "file", default_value = "wallet.key")]
        files: Vec<PathBuf>,
    },
    /// Create a new wallet
    Create(CreateCmd),
    /// Get the balance for a wallet
    Balance {
        /// Wallet(s) to read addresses from
        #[structopt(short = "f", long = "file")]
        files: Vec<PathBuf>,
        /// Addresses to get balances for
        #[structopt(short = "a", long = "address")]
        addresses: Vec<String>,
    },
    Hotspots {
        /// Wallet(s) to read addresses from
        #[structopt(short = "f", long = "file")]
        files: Vec<PathBuf>,
        /// Addresses to get balances for
        #[structopt(short = "a", long = "address")]
        addresses: Vec<String>,
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

fn run(cli: Cli) -> Result {
    match cli {
        Cli::Info { files } => cmd_info::cmd_info(files),
        Cli::Verify { files } => {
            let pass = get_password(false)?;
            cmd_verify::cmd_verify(files, &pass)
        }
        Cli::Create(CreateCmd::Basic {
            output,
            force,
            iterations,
        }) => {
            let pass = get_password(true)?;
            cmd_create::cmd_basic(&pass, iterations, output, force)
        }
        Cli::Create(CreateCmd::Sharded {
            output,
            force,
            iterations,
            key_share_count,
            recovery_threshold,
        }) => {
            let pass = get_password(true)?;
            cmd_create::cmd_sharded(
                &pass,
                key_share_count,
                recovery_threshold,
                iterations,
                output,
                force,
            )
        }
        Cli::Balance { files, addresses } => {
            cmd_balance::cmd_balance(collect_addresses(files, addresses)?)
        }
        Cli::Hotspots { files, addresses } => {
            cmd_hotspots::cmd_hotspots(collect_addresses(files, addresses)?)
        }
    }
}

fn collect_addresses(files: Vec<PathBuf>, addresses: Vec<String>) -> Result<Vec<String>> {
    // If no files or addresses are given use the default wallet
    let file_list = if files.len() == 0 && addresses.len() == 0 {
        vec![PathBuf::from("wallet.key")]
    } else {
        files
    };
    let mut address_list = addresses.clone();
    for file in file_list {
        let mut reader = fs::File::open(&file)?;
        let enc_wallet = Wallet::read(&mut reader)?;
        address_list.push(enc_wallet.public_key().to_b58()?);
    }
    Ok(address_list)
}
