use crate::{
    keypair::PubKeyBin,
    mnemonic,
    result::Result,
    traits::{ReadWrite, B58},
    wallet::Wallet,
};
use std::{env, fs, path::PathBuf};
use structopt::{clap::arg_enum, StructOpt};

pub mod balance;
pub mod create;
pub mod hotspots;
pub mod htlc;
pub mod info;
pub mod onboard;
pub mod oracle;
pub mod oui;
pub mod pay;
pub mod verify;

arg_enum! {
    #[derive(Debug)]
    pub enum OutputFormat {
        Table,
        Json,
    }
}

/// Common options for most wallet commands
#[derive(Debug, StructOpt)]
pub struct Opts {
    /// File(s) to use
    #[structopt(
        short = "f",
        long = "file",
        number_of_values(1),
        default_value = "wallet.key"
    )]
    files: Vec<PathBuf>,

    /// Output formwat to use
    #[structopt(long = "format",
                possible_values = &["table", "json"],
                case_insensitive = true,
                default_value = "table")]
    format: OutputFormat,
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

fn get_password(confirm: bool) -> std::io::Result<String> {
    match env::var("HELIUM_WALLET_PASSWORD") {
        Ok(str) => Ok(str),
        _ => {
            use dialoguer::Password;
            let mut builder = Password::new();
            builder.with_prompt("Password");
            if confirm {
                builder.with_confirmation("Confirm password", "Passwords do not match");
            };
            builder.interact()
        }
    }
}

fn api_url() -> String {
    env::var("HELIUM_API_URL").unwrap_or_else(|_| helium_api::DEFAULT_BASE_URL.to_string())
}

fn collect_addresses(files: Vec<PathBuf>, mut addresses: Vec<String>) -> Result<Vec<String>> {
    // Any given addresses override _all_ the file parameters
    if addresses.is_empty() {
        for file in files {
            let mut reader = fs::File::open(&file)?;
            let enc_wallet = Wallet::read(&mut reader)?;
            addresses.push(enc_wallet.address()?);
        }
    }
    Ok(addresses)
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

pub fn get_payer(staking_address: PubKeyBin, payer: &Option<String>) -> Result<Option<PubKeyBin>> {
    match payer {
        Some(s) if s == "staking" => Ok(Some(staking_address)),
        Some(s) => {
            let address = PubKeyBin::from_b58(&s)?;
            Ok(Some(address))
        }
        None => Ok(None),
    }
}
