use crate::{
    cmd::*,
    keypair::{KeyTag, KeyType, Network},
    mnemonic::SeedType,
    result::Result,
    wallet::{ShardConfig, Wallet},
};
use std::path::PathBuf;

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

    #[structopt(long, possible_values = &["bip39", "mobile"], case_insensitive = true, conflicts_with = "swarm")]
    /// Use a BIP39 or mobile app seed phrase to generate the wallet keys
    seed: Option<SeedType>,

    #[structopt(long, conflicts_with = "swarm")]
    /// The network to generate the wallet (testnet/mainnet) [default: mainnet]
    network: Option<Network>,

    #[structopt(long, conflicts_with = "swarm")]
    /// The type of key to generate (ecc_compact/ed25519) [default: ed25519]
    key_type: Option<KeyType>,

    #[structopt(long)]
    /// Import a swarm_key file from a miner, gateway, validator, or other
    /// blockchain actor.
    swarm: Option<PathBuf>,
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

    #[structopt(short = "n", long = "shards", default_value = "5")]
    /// Number of shards to break the key into
    key_share_count: u8,

    #[structopt(short = "k", long = "required-shards", default_value = "3")]
    /// Number of shards required to recover the key
    recovery_threshold: u8,

    #[structopt(long, possible_values = &["bip39", "mobile"], case_insensitive = true, conflicts_with = "swarm")]
    /// Use a BIP39 or mobile app seed phrase to generate the wallet keys
    seed: Option<SeedType>,

    #[structopt(long, conflicts_with = "swarm")]
    /// The network to generate the wallet (testnet/mainnet) [default: mainnet]
    network: Option<Network>,

    #[structopt(long, conflicts_with = "swarm")]
    /// The type of key to generate (ecc_compact/ed25519) [default: ed25519]
    key_type: Option<KeyType>,

    #[structopt(long)]
    /// Import a swarm_key file from a miner, gateway, validator, or other
    /// blockchain actor.
    swarm: Option<PathBuf>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Basic(cmd) => cmd.run(opts).await,
            Cmd::Sharded(cmd) => cmd.run(opts).await,
        }
    }
}

impl Basic {
    pub async fn run(&self, opts: Opts) -> Result {
        let seed_words = match &self.seed {
            Some(seed_type) => Some(get_seed_words(seed_type)?),
            None => None,
        };
        let password = get_password(true)?;

        let mut builder = Wallet::builder()
            .output(&self.output)
            .password(&password)
            .force(self.force);

        builder = match (self.network, self.key_type, &self.swarm) {
            (_, _, Some(swarm)) => builder.from_swarm(swarm.to_path_buf()),
            (network, key_type, None) => {
                let tag = KeyTag {
                    network: network.unwrap_or(Network::MainNet),
                    key_type: key_type.unwrap_or(KeyType::Ed25519),
                };
                builder
                    .key_tag(&tag)
                    .seed_type(self.seed.to_owned())
                    .seed_words(seed_words)
            }
        };
        let wallet = builder.create()?;

        verify::print_result(
            &wallet,
            &wallet.decrypt(password.as_bytes()),
            None,
            opts.format,
        )
    }
}

impl Sharded {
    pub async fn run(&self, opts: Opts) -> Result {
        let seed_words = match &self.seed {
            Some(seed_type) => Some(get_seed_words(seed_type)?),
            None => None,
        };
        let password = get_password(true)?;

        let shard_config = ShardConfig {
            key_share_count: self.key_share_count,
            recovery_threshold: self.recovery_threshold,
        };

        let mut builder = Wallet::builder()
            .output(&self.output)
            .password(&password)
            .force(self.force)
            .shard(Some(shard_config));

        builder = match (self.network, self.key_type, &self.swarm) {
            (_, _, Some(swarm)) => builder.from_swarm(swarm.to_path_buf()),
            (network, key_type, None) => {
                let tag = KeyTag {
                    network: network.unwrap_or(Network::MainNet),
                    key_type: key_type.unwrap_or(KeyType::Ed25519),
                };
                builder
                    .key_tag(&tag)
                    .seed_type(self.seed.to_owned())
                    .seed_words(seed_words)
            }
        };

        let wallet = builder.create()?;

        verify::print_result(
            &wallet,
            &wallet.decrypt(password.as_bytes()),
            None,
            opts.format,
        )
    }
}
