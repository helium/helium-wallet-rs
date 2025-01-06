use crate::{cmd::*, wallet::ShardConfig};
use clap::builder::TypedValueParser as _;
use helium_lib::{bs58, keypair};

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: CreateCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, clap::Subcommand)]
/// Create a new wallet or keypair
pub enum CreateCommand {
    Basic(Basic),
    Sharded(Sharded),
    Keypair(Keypair),
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

    #[arg(long)]
    /// Use solana byte array or b58 encoded private key
    key: bool,
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

    #[arg(long)]
    /// Use solana byte array or b58 encoded private key
    key: bool,
}

#[derive(Debug, clap::Args)]
/// Create a new helium keypair of a given type
pub struct Keypair {
    #[arg(
        default_value_t = helium_crypto::KeyType::Ed25519,
        value_parser = clap::builder::PossibleValuesParser::new(["secp256k1", "ed25519", "ecc_compact"])
            .map(|s| s.parse::<helium_crypto::KeyType>().unwrap()),
    )]
    r#type: helium_crypto::KeyType,
}

impl CreateCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Basic(cmd) => cmd.run(opts).await,
            Self::Sharded(cmd) => cmd.run(opts).await,
            Self::Keypair(cmd) => cmd.run(opts).await,
        }
    }
}

fn get_entropy(seed: bool, key: bool) -> Result<Option<Vec<u8>>> {
    let key = if key {
        Some(get_secret_entropy()?)
    } else {
        None
    };
    let seed = if key.is_none() && seed {
        Some(get_seed_entropy()?)
    } else {
        None
    };

    Ok(key.or(seed))
}

impl Basic {
    pub async fn run(&self, _opts: Opts) -> Result {
        let entropy = get_entropy(self.seed, self.key)?;
        let password = get_wallet_password(true)?;

        let wallet = Wallet::builder()
            .output(&self.output)
            .password(&password)
            .force(self.force)
            .entropy(entropy)
            .create()?;

        info::print_wallet(&wallet)
    }
}

impl Sharded {
    pub async fn run(&self, _opts: Opts) -> Result {
        let entropy = get_entropy(self.seed, self.key)?;
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
            .entropy(entropy)
            .create()?;

        info::print_wallet(&wallet)
    }
}

impl Keypair {
    pub async fn run(&self, _opts: Opts) -> Result {
        let key_tag = helium_crypto::KeyTag {
            network: helium_crypto::Network::MainNet,
            key_type: self.r#type,
        };

        let keypair = helium_crypto::Keypair::generate(key_tag, &mut rand::rngs::OsRng);
        let secret = keypair.secret_to_vec();
        let mut json = json!({
            "type": keypair.key_tag().key_type.to_string(),
            "secret": {
                "bytes": serde_json::to_string(&secret)?,
                "b58": bs58::encode(secret).into_string(),
            }
        });
        let mut public_key = json!({
            "helium": keypair.public_key().to_string(),
        });

        if key_tag.key_type == helium_crypto::KeyType::Ed25519 {
            let solana_key = keypair::to_pubkey(keypair.public_key())?;
            public_key["solana"] = solana_key.to_string().into();
        }
        json["public_key"] = public_key;
        print_json(&json)
    }
}

fn get_seed_entropy() -> Result<Vec<u8>> {
    fn secret_from_phrase(s: &str) -> Result<Vec<u8>> {
        let entropy = helium_mnemonic::mnemonic_to_entropy(&phrase_to_words(s))?.to_vec();
        Ok(entropy)
    }

    match env::var("HELIUM_WALLET_SEED_WORDS") {
        Ok(word_string) => secret_from_phrase(&word_string),
        _ => {
            use dialoguer::Input;
            let word_string = Input::<String>::new()
                .with_prompt("Space separated seed words")
                .validate_with(|v: &String| secret_from_phrase(v.as_str()).map(|_| ()))
                .interact()?;
            secret_from_phrase(&word_string)
        }
    }
}

fn get_secret_entropy() -> Result<Vec<u8>> {
    fn secret_from_str(s: &str) -> Result<Vec<u8>> {
        if s.starts_with('[') {
            serde_json::from_str::<Vec<u8>>(s).map_err(Error::from)
        } else {
            bs58::decode(s).into_vec().map_err(Error::from)
        }
    }

    match env::var("HELIUM_WALLET_SECRET") {
        Ok(secret) => secret_from_str(&secret),
        _ => {
            use dialoguer::Input;
            let secret_string = Input::<String>::new()
                .with_prompt("Solana secret")
                .validate_with(|v: &String| secret_from_str(v.as_str()).map(|_| ()))
                .interact()?;
            secret_from_str(&secret_string)
        }
    }
}
