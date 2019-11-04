use crate::{
    cmd_verify,
    result::Result,
    traits::ReadWrite,
    wallet::{basic::BasicWallet, sharded::ShardedWallet},
};
use std::{fs::OpenOptions, path::PathBuf};

pub fn cmd_basic(password: &str, iterations: u32, output: PathBuf, force: bool) -> Result {
    let enc_wallet = BasicWallet::create(iterations, password.as_bytes())?;

    let mut writer = OpenOptions::new()
        .write(true)
        .create(true)
        .create_new(!force)
        .open(output.clone())?;

    enc_wallet.write(&mut writer)?;
    crate::cmd_verify::cmd_verify(vec![output], password)
}

pub fn cmd_sharded(
    password: &str,
    key_share_count: u8,
    recovery_threshold: u8,
    iterations: u32,
    output: PathBuf,
    force: bool,
) -> Result {
    let enc_wallets = ShardedWallet::create(
        iterations,
        key_share_count,
        recovery_threshold,
        password.as_bytes(),
    )?;
    println!("num wallet {}", enc_wallets.len());
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
