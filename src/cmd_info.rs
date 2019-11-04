use crate::{
    traits::{ReadWrite, B58},
    wallet::{Wallet, WalletReadWrite},
};
use std::{error::Error, fs, path::PathBuf, result::Result};

pub fn cmd_info(files: Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for file in files {
        let mut reader = fs::File::open(&file)?;
        let enc_wallet = WalletReadWrite::read(&mut reader)?;
        match enc_wallet {
            WalletReadWrite::Basic(wallet) => {
                println!("Address: {}", wallet.public_key().to_b58()?);
                println!("File: {}", file.display());
            }
            WalletReadWrite::Sharded(wallet) => {
                println!("Address: {}", wallet.public_key().to_b58()?);
                println!("Shards: {}", wallet.num_shards());
                println!("File: {}", file.display());
            }
        }
    }
    Ok(())
}
