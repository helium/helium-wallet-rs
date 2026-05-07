use crate::{cmd::*, wallet::ShardConfig};
use clap::builder::TypedValueParser as _;
use helium_lib::{
    bs58,
    keypair::{self, Signer},
};
use rand::{RngCore, SeedableRng};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const ATTEMPT_BATCH_SIZE: u64 = 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);

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
#[command(group(
    clap::ArgGroup::new("grind_pattern")
        .args(["starts_with", "ends_with"])
        .multiple(true),
))]
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

    #[arg(long, value_name = "PREFIX", conflicts_with_all = ["seed", "key"])]
    /// Grind until the Solana address starts with this base58 prefix
    starts_with: Option<String>,

    #[arg(long, value_name = "SUFFIX", conflicts_with_all = ["seed", "key"])]
    /// Grind until the Solana address ends with this base58 suffix
    ends_with: Option<String>,

    #[arg(long, requires = "grind_pattern")]
    /// Match grind prefix and suffix without ASCII case sensitivity
    ignore_case: bool,

    #[arg(long, value_name = "COUNT", requires = "grind_pattern")]
    /// Number of grind threads to use; defaults to available parallelism
    num_threads: Option<usize>,
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
        if self.is_grind() {
            return self.run_grind().await;
        }

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

    async fn run_grind(&self) -> Result {
        let num_threads = self.num_threads.unwrap_or_else(default_num_threads);
        if num_threads == 0 {
            bail!("--num-threads must be greater than zero");
        }
        if !self.force && self.output.exists() {
            bail!(
                "{} already exists; use --force to overwrite",
                self.output.display()
            );
        }

        let pattern = GrindPattern::new(
            self.starts_with.clone(),
            self.ends_with.clone(),
            self.ignore_case,
        )?;
        let GrindResult {
            entropy,
            attempts,
            elapsed,
        } = grind_keypair(pattern, num_threads)?;

        let password = get_wallet_password(true)?;
        let wallet = Wallet::builder()
            .output(&self.output)
            .password(&password)
            .force(self.force)
            .entropy(Some(entropy))
            .create()?;

        print_grind_wallet(&wallet, attempts, elapsed)
    }

    fn is_grind(&self) -> bool {
        self.starts_with.is_some() || self.ends_with.is_some()
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

#[derive(Clone, Debug)]
struct GrindPattern {
    starts_with: Option<String>,
    ends_with: Option<String>,
    ignore_case: bool,
}

#[derive(Debug)]
struct GrindResult {
    entropy: Vec<u8>,
    attempts: u64,
    elapsed: Duration,
}

impl GrindPattern {
    fn new(
        starts_with: Option<String>,
        ends_with: Option<String>,
        ignore_case: bool,
    ) -> Result<Self> {
        match (starts_with.as_deref(), ends_with.as_deref()) {
            (None, None) => bail!("at least one of --starts-with or --ends-with is required"),
            (Some(""), _) => bail!("--starts-with must not be empty"),
            (_, Some("")) => bail!("--ends-with must not be empty"),
            _ => {}
        }

        if let Some(prefix) = &starts_with {
            validate_base58_pattern("--starts-with", prefix, ignore_case)?;
        }
        if let Some(suffix) = &ends_with {
            validate_base58_pattern("--ends-with", suffix, ignore_case)?;
        }

        let combined_len =
            starts_with.as_ref().map_or(0, String::len) + ends_with.as_ref().map_or(0, String::len);
        if combined_len > 44 {
            bail!("combined prefix and suffix length cannot exceed 44 characters");
        }

        Ok(Self {
            starts_with,
            ends_with,
            ignore_case,
        })
    }

    fn matches(&self, pubkey: &str) -> bool {
        self.matches_start(pubkey) && self.matches_end(pubkey)
    }

    fn matches_start(&self, pubkey: &str) -> bool {
        match &self.starts_with {
            Some(prefix) => pubkey
                .get(..prefix.len())
                .is_some_and(|value| pattern_eq(value, prefix, self.ignore_case)),
            None => true,
        }
    }

    fn matches_end(&self, pubkey: &str) -> bool {
        match &self.ends_with {
            Some(suffix) => pubkey
                .get(pubkey.len().saturating_sub(suffix.len())..)
                .is_some_and(|value| pattern_eq(value, suffix, self.ignore_case)),
            None => true,
        }
    }
}

fn grind_keypair(pattern: GrindPattern, num_threads: usize) -> Result<GrindResult> {
    let pattern = Arc::new(pattern);
    let found = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>>>();
    let mut handles = Vec::with_capacity(num_threads);
    let start = Instant::now();
    let mut last_progress = start;
    let mut last_attempts = 0;

    for _ in 0..num_threads {
        let pattern = Arc::clone(&pattern);
        let found = Arc::clone(&found);
        let attempts = Arc::clone(&attempts);
        let tx = tx.clone();

        handles.push(thread::spawn(move || {
            let mut rng = rand::rngs::StdRng::from_entropy();
            let mut pending_attempts = 0;

            while !found.load(Ordering::Relaxed) {
                let mut entropy = [0u8; 32];
                rng.fill_bytes(&mut entropy);

                let keypair = match keypair::Keypair::generate_from_entropy(&entropy) {
                    Ok(keypair) => keypair,
                    Err(err) => {
                        flush_pending_attempts(attempts.as_ref(), &mut pending_attempts);
                        found.store(true, Ordering::Relaxed);
                        let _ = tx.send(Err(err.into()));
                        return;
                    }
                };
                let pubkey = keypair.pubkey();
                pending_attempts += 1;
                if pending_attempts >= ATTEMPT_BATCH_SIZE {
                    flush_pending_attempts(attempts.as_ref(), &mut pending_attempts);
                }

                if pattern.matches(&pubkey.to_string()) {
                    flush_pending_attempts(attempts.as_ref(), &mut pending_attempts);
                    if !found.swap(true, Ordering::Relaxed) {
                        let _ = tx.send(Ok(entropy.to_vec()));
                    }
                    return;
                }
            }
            flush_pending_attempts(attempts.as_ref(), &mut pending_attempts);
        }));
    }
    drop(tx);

    let result = loop {
        match rx.recv_timeout(PROGRESS_INTERVAL) {
            Ok(result) => break result,
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                let current_attempts = attempts.load(Ordering::Relaxed);
                print_grind_progress(current_attempts, last_attempts, start, last_progress, now);
                last_attempts = current_attempts;
                last_progress = now;
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(anyhow!("grind threads exited without a result"));
            }
        }
    };
    for handle in handles {
        handle
            .join()
            .map_err(|_| anyhow!("grind thread panicked"))?;
    }
    let entropy = result?;
    let elapsed = start.elapsed();
    let total_attempts = attempts.load(Ordering::Relaxed);

    Ok(GrindResult {
        entropy,
        attempts: total_attempts,
        elapsed,
    })
}

fn flush_pending_attempts(attempts: &AtomicU64, pending_attempts: &mut u64) {
    if *pending_attempts > 0 {
        attempts.fetch_add(*pending_attempts, Ordering::Relaxed);
        *pending_attempts = 0;
    }
}

fn print_grind_progress(
    current_attempts: u64,
    last_attempts: u64,
    start: Instant,
    last_progress: Instant,
    now: Instant,
) {
    let recent_rate = rate_since(
        current_attempts.saturating_sub(last_attempts),
        last_progress,
        now,
    );
    let average_rate = rate_since(current_attempts, start, now);
    eprintln!(
        "Searched {} keys ({}/s recent, {}/s avg)",
        current_attempts,
        format_rate(recent_rate),
        format_rate(average_rate)
    );
}

fn print_grind_wallet(wallet: &Wallet, attempts: u64, elapsed: Duration) -> Result {
    let mut json = info::wallet_json(wallet)?;
    json["grind"] = json!({
        "attempts": attempts,
        "elapsed_seconds": elapsed.as_secs_f64(),
        "keys_per_second": rate_for_duration(attempts, elapsed),
    });
    print_json(&json)
}

fn rate_since(attempts: u64, start: Instant, end: Instant) -> f64 {
    rate_for_duration(attempts, end.duration_since(start))
}

fn rate_for_duration(attempts: u64, duration: Duration) -> f64 {
    let seconds = duration.as_secs_f64();
    if seconds > 0.0 {
        attempts as f64 / seconds
    } else {
        0.0
    }
}

fn format_rate(rate: f64) -> String {
    if rate >= 1_000_000.0 {
        format!("{:.1}M", rate / 1_000_000.0)
    } else if rate >= 1_000.0 {
        format!("{:.1}K", rate / 1_000.0)
    } else {
        format!("{rate:.0}")
    }
}

fn default_num_threads() -> usize {
    thread::available_parallelism().map_or(1, |threads| threads.get())
}

fn validate_base58_pattern(name: &str, value: &str, ignore_case: bool) -> Result {
    let is_valid = if ignore_case {
        value.chars().all(is_base58_case_variant)
    } else {
        bs58::decode(value).into_vec().is_ok()
    };

    if is_valid {
        Ok(())
    } else {
        bail!("{name} must contain only base58 characters");
    }
}

fn is_base58_case_variant(ch: char) -> bool {
    is_base58_char(ch)
        || (ch.is_ascii_alphabetic()
            && (is_base58_char(ch.to_ascii_lowercase()) || is_base58_char(ch.to_ascii_uppercase())))
}

fn is_base58_char(ch: char) -> bool {
    if !ch.is_ascii() {
        return false;
    }

    let mut decoded = [0u8; 1];
    bs58::decode([ch as u8]).onto(&mut decoded).is_ok()
}

fn pattern_eq(value: &str, pattern: &str, ignore_case: bool) -> bool {
    if ignore_case {
        value.eq_ignore_ascii_case(pattern)
    } else {
        value == pattern
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grind_pattern_requires_a_matcher() {
        GrindPattern::new(None, None, false).expect_err("missing matcher should fail");
    }

    #[test]
    fn grind_pattern_matches_prefix_and_suffix() {
        let pattern = GrindPattern::new(Some("abc".to_string()), Some("XYZ".to_string()), false)
            .expect("valid pattern");

        assert!(pattern.matches("abcdefXYZ"));
        assert!(!pattern.matches("abCdefXYZ"));
        assert!(!pattern.matches("abcdefXYz"));
    }

    #[test]
    fn grind_pattern_can_ignore_case() {
        let pattern = GrindPattern::new(Some("abc".to_string()), Some("xyz".to_string()), true)
            .expect("valid pattern");

        assert!(pattern.matches("ABCdefXYZ"));
    }

    #[test]
    fn grind_pattern_ignore_case_accepts_base58_case_variant() {
        GrindPattern::new(Some("O".to_string()), None, true)
            .expect("capital o can match lowercase");
    }

    #[test]
    fn grind_pattern_rejects_impossible_base58() {
        GrindPattern::new(Some("0".to_string()), None, false).expect_err("zero is not base58");
        GrindPattern::new(Some("0".to_string()), None, true).expect_err("zero is not base58");
    }
}
