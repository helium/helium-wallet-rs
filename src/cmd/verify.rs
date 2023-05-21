use crate::{cmd::*, keypair::Keypair, result::Result, wallet::Wallet};
use serde_json::json;
use std::rc::Rc;

/// Verify an encypted wallet
#[derive(Debug, clap::Args)]
pub struct Cmd {}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let decryped_wallet = wallet.decrypt(password.as_bytes());
        print_result(&wallet, &decryped_wallet)
    }
}

pub fn print_result(wallet: &Wallet, decrypted_wallet: &Result<Rc<Keypair>>) -> Result {
    let address = wallet.address().unwrap_or_else(|_| "unknown".to_string());
    let phrase = decrypted_wallet
        .as_ref()
        .map_or(None, |kp| kp.phrase().ok());

    let json = json!({
        "address": address,
        "sharded": wallet.is_sharded(),
        "verify": decrypted_wallet.is_ok(),
        "pwhash": wallet.pwhash().to_string(),
        "phrase": phrase,
    });
    print_json(&json)
}
