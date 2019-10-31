use crate::{
    keypair::PublicKey,
    result::Result,
    traits::{ReadWrite, B58},
    wallet::{self, Wallet},
};
use std::{fs, path::PathBuf};

pub fn cmd_verify(files: Vec<PathBuf>, password: &str) -> Result<()> {
    let first_file = files.first().expect("At least one file expected");
    let is_sharded = {
        let mut reader = fs::File::open(first_file)?;
        let wallet = Wallet::read(&mut reader)?;
        wallet.is_sharded()
    };

    if is_sharded {
        let mut enc_wallets = Vec::new();
        for file in files.iter() {
            let mut reader = fs::File::open(&file)?;
            let enc_wallet = Wallet::read(&mut reader)?;
            enc_wallets.push(enc_wallet);
        }
        let result = wallet::Wallet::decrypt_sharded(password.as_bytes(), &enc_wallets);
        print_wallet(
            enc_wallets.first().unwrap().public_key(),
            is_sharded,
            files,
            Some(result),
        )?;
        Ok(())
    } else {
        for file in files.iter() {
            let mut reader = fs::File::open(&file)?;
            let enc_wallet = Wallet::read(&mut reader)?;
            let result = wallet::Wallet::decrypt_basic(password.as_bytes(), &enc_wallet);
            print_wallet(
                enc_wallet.public_key(),
                is_sharded,
                vec![file.clone()],
                Some(result),
            )?;
        }
        Ok(())
    }
}

fn print_wallet(
    public_key: &PublicKey,
    sharded: bool,
    files: Vec<PathBuf>,
    verify: Option<Result<Wallet>>,
) -> Result<()> {
    let file_names: Vec<String> = files.iter().map(|pb| pb.display().to_string()).collect();
    println!("Address: {}", public_key.to_b58()?);
    println!("Sharded: {}", sharded);
    println!("File(s): {}", file_names.join(", "));
    if let Some(result) = verify {
        let msg = match result {
            Ok(_) => "true".to_string(),
            Err(m) => m.to_string()
        };
        println!("Verify: {}", msg);
    };
    Ok(())
}
