use crate::{
    cmd_verify,
    keypair::{Keypair, Seed},
    mnemonic::mnemonic_to_entropy,
    result::Result,
    traits::ReadWrite,
    wallet::{basic, sharded, Wallet},
};
use std::{fs::OpenOptions, path::PathBuf};

pub fn cmd_basic(
    password: &str,
    iterations: u32,
    output: PathBuf,
    force: bool,
    seed_words: Option<Vec<String>>,
) -> Result {
    let keypair = gen_keypair(seed_words)?;
    let wallet = Wallet::Basic(basic::Wallet::Decrypted {
        keypair,
        iterations,
    });
    let enc_wallet = wallet.encrypt(password.as_bytes())?;

    let mut writer = OpenOptions::new()
        .write(true)
        .create(true)
        .create_new(!force)
        .open(output.clone())?;

    enc_wallet[0].write(&mut writer)?;
    crate::cmd_verify::cmd_verify(vec![output], password)?;
    Ok(())
}

pub fn cmd_sharded(
    password: &str,
    key_share_count: u8,
    recovery_threshold: u8,
    iterations: u32,
    output: PathBuf,
    force: bool,
    seed_words: Option<Vec<String>>,
) -> Result {
    let keypair = gen_keypair(seed_words)?;

    let wallet = Wallet::Sharded(sharded::Wallet::Decrypted {
        iterations,
        keypair,
        key_share_count,
        recovery_threshold,
    });
    let enc_wallets = wallet.encrypt(password.as_bytes())?;

    use std::ffi::OsStr;
    let extension: &str = output
        .extension()
        .unwrap_or_else(|| OsStr::new(""))
        .to_str()
        .unwrap();
    let mut filenames = Vec::new();
    for (i, w) in enc_wallets.iter().enumerate() {
        let mut filename = output.clone();
        let share_extension = format!("{}.{}", extension, (i + 1).to_string());
        filename.set_extension(share_extension);
        filenames.push(filename.to_owned());
        let mut writer = OpenOptions::new()
            .write(true)
            .create(true)
            .create_new(!force)
            .open(filename)?;
        w.write(&mut writer)?;
    }
    cmd_verify::cmd_verify(filenames, password)
}

fn gen_keypair(seed_words: Option<Vec<String>>) -> Result<Keypair> {
    match seed_words {
        Some(words) => {
            let entropy = mnemonic_to_entropy(words)?;
            Ok(Keypair::gen_keypair_from_seed(&Seed(entropy)))
        }
        None => Ok(Keypair::gen_keypair()),
    }
}
