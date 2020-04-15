use crate::{
    cmd::{get_password, get_seed_words, Opts},
    keypair::{Keypair, Seed},
    mnemonic::mnemonic_to_entropy,
    result::Result,
    traits::ReadWrite,
    wallet::{BasicFormat, ShardedFormat, Wallet},
};
use std::{fs::OpenOptions, path::PathBuf};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Create a new wallet
pub enum Cmd {
    Basic(Basic),
    Sharded(Sharded),
}

#[derive(Debug, StructOpt)]
/// Create a new basic wallet
pub struct Basic {
    #[structopt(short, long, default_value = "wallet.key")]
    /// Output file to store the key in
    output: PathBuf,

    #[structopt(long)]
    /// Overwrite an existing file
    force: bool,

    #[structopt(short = "i", long = "iterations", default_value = "1000000")]
    /// Number of PBKDF2 interations
    iterations: u32,

    #[structopt(long)]
    /// Use seed words to create the wallet
    seed: bool,
}

#[derive(Debug, StructOpt)]
/// Create a new sharded wallet
pub struct Sharded {
    #[structopt(short, long, default_value = "wallet.key")]
    /// Output file to store the key in
    output: PathBuf,

    #[structopt(long)]
    /// Overwrite an existing file
    force: bool,

    #[structopt(short = "i", long = "iterations", default_value = "1000000")]
    /// Number of PBKDF2 interations
    iterations: u32,

    #[structopt(short = "n", long = "shards", default_value = "5")]
    /// Number of shards to break the key into
    key_share_count: u8,

    #[structopt(short = "k", long = "required-shards", default_value = "3")]
    /// Number of shards required to recover the key
    recovery_threshold: u8,

    #[structopt(long)]
    /// Use seed words to create the wallet
    seed: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Basic(cmd) => cmd.run(opts),
            Cmd::Sharded(cmd) => cmd.run(opts),
        }
    }
}

impl Basic {
    pub fn run(&self, _opts: Opts) -> Result {
        let seed_words = if self.seed {
            Some(get_seed_words()?)
        } else {
            None
        };
        let password = get_password(true)?;
        let keypair = gen_keypair(seed_words)?;
        let mut format = BasicFormat::default();
        let wallet =
            Wallet::from_keypair(&keypair, password.as_bytes(), self.iterations, &mut format)?;

        let mut writer = OpenOptions::new()
            .write(true)
            .create(true)
            .create_new(!self.force)
            .open(self.output.clone())?;

        wallet.write(&mut writer)?;
        //    crate::cmd_verify::cmd_verify(&wallet, password)?;
        Ok(())
    }
}

impl Sharded {
    pub fn run(&self, _opts: Opts) -> Result {
        let seed_words = if self.seed {
            Some(get_seed_words()?)
        } else {
            None
        };
        let password = get_password(true)?;

        let keypair = gen_keypair(seed_words)?;
        let mut format = ShardedFormat {
            key_share_count: self.key_share_count,
            recovery_threshold: self.recovery_threshold,
            key_shares: vec![],
        };
        let mut wallet =
            Wallet::from_keypair(&keypair, password.as_bytes(), self.iterations, &mut format)?;

        use std::ffi::OsStr;
        let extension: &str = self
            .output
            .extension()
            .unwrap_or_else(|| OsStr::new(""))
            .to_str()
            .unwrap();
        let mut filenames = Vec::new();
        for (i, share) in format.key_shares.iter().enumerate() {
            let mut filename = self.output.clone();
            let share_extension = format!("{}.{}", extension, (i + 1).to_string());
            filename.set_extension(share_extension);
            filenames.push(filename.to_owned());
            let mut writer = OpenOptions::new()
                .write(true)
                .create(true)
                .create_new(!self.force)
                .open(filename)?;
            wallet.format = Box::new(ShardedFormat {
                key_shares: vec![share.clone()],
                ..format
            });
            wallet.write(&mut writer)?;
        }
        wallet.format = Box::new(format);
        // cmd_verify::cmd_verify(&wallet, password)
        Ok(())
    }
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
