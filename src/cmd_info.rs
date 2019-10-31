use crate::{
    traits::{ReadWrite, B58},
    wallet::Wallet,
};
use std::{error::Error, fs, path::PathBuf, result::Result};

pub fn cmd_info(files: Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for file in files {
        let mut reader = fs::File::open(&file)?;
        let enc_wallet = Wallet::read(&mut reader)?;
        println!("Address: {}", enc_wallet.public_key().to_b58()?);
        println!("Sharded: {}", enc_wallet.is_sharded());
        println!("File: {}", file.display());
    }
    Ok(())
}
