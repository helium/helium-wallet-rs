use crate::{
    cmd::{print_json, Opts},
    result::{Error, Result},
    wallet::Wallet,
};
use helium_lib::keypair::{to_helium_pubkey, to_pubkey, Pubkey};
use qr2term::print_qr;
use serde_json::json;
use std::str::FromStr;

/// Get wallet information
#[derive(Debug, clap::Args)]
pub struct Cmd {
    /// Display QR code for a given single wallet.
    #[arg(long)]
    qr: bool,

    /// Wallet address (Solana or Helium format). If not provided, uses the wallet keyfile.
    address: Option<String>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        if let Some(address) = &self.address {
            let pubkey = parse_address(address)?;
            print_address(&pubkey)
        } else {
            let wallet = opts.load_wallet()?;
            if self.qr {
                print_qr(wallet.public_key.to_string()).map_err(Error::from)
            } else {
                print_wallet(&wallet)
            }
        }
    }
}

fn parse_address(address: &str) -> Result<Pubkey> {
    Pubkey::from_str(address).or_else(|_| {
        let helium_pubkey = helium_crypto::PublicKey::from_str(address)?;
        to_pubkey(&helium_pubkey).map_err(Error::from)
    })
}

fn print_address(address: &Pubkey) -> Result {
    let helium_address = to_helium_pubkey(address)?;
    let json = json!({
        "address": {
            "solana": address.to_string(),
            "helium": helium_address.to_string(),
        },
    });
    print_json(&json)
}

pub(crate) fn print_wallet(wallet: &Wallet) -> Result {
    let helium_address = wallet.helium_address()?;
    let address = wallet.address()?;
    let json = json!({
        "sharded": wallet.is_sharded(),
        "pwhash": wallet.pwhash().to_string(),
        "address": {
            "solana": address,
            "helium": helium_address,
        },
    });
    print_json(&json)
}
