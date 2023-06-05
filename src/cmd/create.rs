use crate::{
    cmd::*,
    result::Result,
    wallet::{ShardConfig, Wallet},
};
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: CreateCommand,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts)
    }
}

#[derive(Debug, clap::Subcommand)]
/// Create a new wallet
pub enum CreateCommand {
    Basic(Basic),
    Sharded(Sharded),
}

#[derive(Debug, clap::Args)]
/// Create a new basic wallet
pub struct Basic {
    #[arg(short, long, default_value = "wallet.key")]
    /// Output file to store the key in
    output: PathBuf,

    #[arg(long)]
    /// Overwrite an existing file
    force: bool,

    #[arg(long)]
    /// Use a BIP39 or mobile app seed phrase to generate the wallet keys
    seed: bool,
}

#[derive(Debug, clap::Args)]
/// Create a new sharded wallet
pub struct Sharded {
    #[arg(short, long, default_value = "wallet.key")]
    /// Output file to store the key in
    output: PathBuf,

    #[arg(long)]
    /// Overwrite an existing file
    force: bool,

    #[arg(short = 'n', long = "shards", default_value = "5")]
    /// Number of shards to break the key into
    key_share_count: u8,

    #[arg(short = 'k', long = "required-shards", default_value = "3")]
    /// Number of shards required to recover the key
    recovery_threshold: u8,

    #[arg(long)]
    /// Use a BIP39 or mobile app seed phrase to generate the wallet keys
    seed: bool,
}

impl CreateCommand {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Basic(cmd) => cmd.run(opts),
            Self::Sharded(cmd) => cmd.run(opts),
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
        let password = get_wallet_password(true)?;

        let wallet = Wallet::builder()
            .output(&self.output)
            .password(&password)
            .force(self.force)
            .seed_phrase(seed_words)
            .create()?;

        info::print_wallet(&wallet)
    }
}

impl Sharded {
    pub fn run(&self, _opts: Opts) -> Result {
        let seed_words = self.seed.then_some(get_seed_words()?);
        let password = get_wallet_password(true)?;

        let shard_config = ShardConfig {
            key_share_count: self.key_share_count,
            recovery_threshold: self.recovery_threshold,
        };

        let wallet = Wallet::builder()
            .output(&self.output)
            .password(&password)
            .force(self.force)
            .shard(Some(shard_config))
            .seed_phrase(seed_words)
            .create()?;

        info::print_wallet(&wallet)
    }
}

fn get_seed_words() -> Result<String> {
    use bip39::{Language, Mnemonic};
    match env::var("HELIUM_WALLET_SEED_WORDS") {
        Ok(phrase) => Ok(phrase),
        _ => {
            use dialoguer::Input;
            let phrase = Input::<String>::new()
                .with_prompt("Space separated seed words")
                .validate_with(|phrase: &String| {
                    match Mnemonic::from_phrase(phrase, Language::English) {
                        Ok(_) => Ok(()),
                        Err(err) => Err(err),
                    }
                })
                .interact()?;
            Ok(phrase)
        }
    }
}
