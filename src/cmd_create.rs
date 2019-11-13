use crate::{
    cmd_verify,
    keypair::{Keypair, Seed},
    mnemonic::mnemonic_to_entropy,
    result::Result,
    traits::ReadWrite,
    wallet::{Wallet, BasicFormat, ShardedFormat},
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
    let mut format = BasicFormat::default();
    let wallet = Wallet::from_keypair(&keypair, password.as_bytes(), iterations, &mut format)?;

    let mut writer = OpenOptions::new()
        .write(true)
        .create(true)
        .create_new(!force)
        .open(output.clone())?;

    wallet.write(&mut writer)?;
    crate::cmd_verify::cmd_verify(&wallet, password)?;
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
    let mut format = ShardedFormat {key_share_count, recovery_threshold, key_shares: vec![]};
    let mut wallet = Wallet::from_keypair(&keypair, password.as_bytes(), iterations, &mut format)?;

    use std::ffi::OsStr;
    let extension: &str = output
        .extension()
        .unwrap_or_else(|| OsStr::new(""))
        .to_str()
        .unwrap();
    let mut filenames = Vec::new();
    for (i, share) in format.key_shares.iter().enumerate() {
        let mut filename = output.clone();
        let share_extension = format!("{}.{}", extension, (i + 1).to_string());
        filename.set_extension(share_extension);
        filenames.push(filename.to_owned());
        let mut writer = OpenOptions::new()
            .write(true)
            .create(true)
            .create_new(!force)
            .open(filename)?;
        wallet.format = Box::new(ShardedFormat{key_shares: vec![share.clone()], ..format});
        wallet.write(&mut writer)?;
    }
    wallet.format = Box::new(format);
    cmd_verify::cmd_verify(&wallet, password)
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
