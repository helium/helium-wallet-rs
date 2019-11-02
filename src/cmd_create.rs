use crate::{
    cmd_verify,
    keypair::Keypair,
    result::Result,
    traits::ReadWrite,
    wallet::{basic::BasicWallet, sharded::ShardedWallet, Wallet},
};
use std::{fs::OpenOptions, path::PathBuf};

pub fn cmd_basic(password: &str, iterations: u32, output: PathBuf, force: bool) -> Result {
    let keypair = Keypair::gen_keypair();
    let wallet = Wallet::Basic(BasicWallet::Decrypted {
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
    let keypair = Keypair::gen_keypair();

    let wallet = Wallet::Sharded(ShardedWallet::Decrypted {
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
