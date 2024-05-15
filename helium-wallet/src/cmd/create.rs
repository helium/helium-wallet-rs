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

impl Basic {
    pub async fn run(&self, _opts: Opts) -> Result {
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
    pub async fn run(&self, _opts: Opts) -> Result {
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

fn get_seed_words() -> Result<Vec<String>> {
    match env::var("HELIUM_WALLET_SEED_WORDS") {
        Ok(word_string) => Ok(phrase_to_words(&word_string)),
        _ => {
            use dialoguer::Input;
            let word_string = Input::<String>::new()
                .with_prompt("Space separated seed words")
                .validate_with(|v: &String| {
                    let word_list = phrase_to_words(v);
                    match helium_mnemonic::mnemonic_to_entropy(word_list) {
                        Ok(_) => Ok(()),
                        Err(err) => Err(err),
                    }
                })
                .interact()?;
            Ok(phrase_to_words(&word_string))
        }
    }
}
