use crate::{
    result::Result,
    traits::{ReadWrite, B58},
    wallet::{self, WalletReadWrite, Wallet},
    keypair::PublicKey,
};
use std::{fs, path::PathBuf};

pub fn cmd_verify(files: Vec<PathBuf>, password: &str) -> Result {
    let first_file = files.first().expect("At least one file expected");
    let mut reader = fs::File::open(first_file)?;
    let wallet_type = WalletReadWrite::read(&mut reader)?;

    match wallet_type {
        WalletReadWrite::Sharded(first_wallet) => {
            let mut enc_wallets = Vec::new();
            for file in files.iter() {
                let mut reader = fs::File::open(&file)?;
                let enc_wallet = WalletReadWrite::read(&mut reader)?;
                match enc_wallet {
                    WalletReadWrite::Sharded(wallet) => {
                        enc_wallets.push(wallet);
                    }
                    WalletReadWrite::Basic(_) => panic!("Basic wallet file mixed with sharded"),
                }
            }
            let result = wallet::decrypt_sharded(password.as_bytes(), enc_wallets);
            print_wallet(
                first_wallet.public_key(),
                true,
                files,
                Some(result),
            )?;
        }
        WalletReadWrite::Basic(_) => {
            for file in files.iter() {
                let mut reader = fs::File::open(&file)?;
                let enc_wallet = WalletReadWrite::read(&mut reader)?;
                match enc_wallet {
                    WalletReadWrite::Basic(wallet) => {
                        let public_key = wallet.public_key().clone();
                        let result = wallet::decrypt_basic(password.as_bytes(), wallet);
                        print_wallet(
                            &public_key,
                            false,
                            vec![file.clone()],
                            Some(result),
                        )?;
                    }
                    WalletReadWrite::Sharded(_) => panic!("Sharded wallet file mixed with basic"),
                }
            }
        }
    }
    Ok(())
}

fn print_wallet<W: Wallet>(
    public_key: &PublicKey,
    sharded: bool,
    files: Vec<PathBuf>,
    verify: Option<Result<W>>,
) -> Result {
    let file_names: Vec<String> = files.iter().map(|pb| pb.display().to_string()).collect();
    println!("Address: {}", public_key.to_b58()?);
    println!("Sharded: {}", sharded);
    println!("File(s): {}", file_names.join(", "));
    if let Some(result) = verify {
        let msg = match result {
            Ok(_) => "true".to_string(),
            Err(m) => m.to_string(),
        };
        println!("Verify: {}", msg);
    };
    Ok(())
}
