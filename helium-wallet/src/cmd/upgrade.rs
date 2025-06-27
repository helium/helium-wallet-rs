use crate::{
    cmd::*,
    format::{self, Format},
    pwhash::PwHash,
};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: UpgradeCmd,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

/// Upgrade a wallet to the latest supported version of the given
/// format. The same password is used to decrypt the old and encrypt
/// the new wallet.
#[derive(Debug, clap::Subcommand)]
pub enum UpgradeCmd {
    Basic(Basic),
    Sharded(Sharded),
}

#[derive(Debug, clap::Args)]
/// Upgrade to the latest basic wallet format
pub struct Basic {
    #[arg(short, long, default_value = "wallet.key")]
    /// Output file to store the key in
    output: PathBuf,

    #[arg(long)]
    /// Overwrite an existing file
    force: bool,
}

#[derive(Debug, clap::Args)]
/// Upgrade to the latest sharded wallet format
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
}

impl UpgradeCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Basic(cmd) => cmd.run(opts).await,
            Self::Sharded(cmd) => cmd.run(opts).await,
        }
    }
}

impl Basic {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;

        let format = format::Basic {
            pwhash: PwHash::argon2id13_default(),
        };
        let new_wallet = Wallet::encrypt(&keypair, password.as_bytes(), Format::Basic(format))?;
        let mut writer = open_output_file(&self.output, !self.force)?;
        new_wallet.write(&mut writer)?;
        info::print_wallet(&new_wallet)
    }
}

impl Sharded {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = opts.load_wallet()?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let format = format::Sharded {
            key_share_count: self.key_share_count,
            recovery_threshold: self.recovery_threshold,
            pwhash: PwHash::argon2id13_default(),
            key_shares: vec![],
        };
        let new_wallet = Wallet::encrypt(&keypair, password.as_bytes(), Format::Sharded(format))?;

        let extension = get_file_extension(&self.output);
        for (i, shard) in new_wallet.shards()?.iter().enumerate() {
            let mut filename = self.output.clone();
            let share_extension = format!("{}.{}", extension, (i + 1));
            filename.set_extension(share_extension);
            let mut writer = open_output_file(&filename, !self.force)?;
            shard.write(&mut writer)?;
        }
        info::print_wallet(&wallet)
    }
}
