use crate::cmd::*;
use helium_lib::keypair::{to_helium_pubkey, Pubkey, Signer};
use serde_json::json;
use sha2::{Digest, Sha256};

/// Solana off-chain message signing domain (16 bytes).
const SIGNING_DOMAIN: &[u8; 16] = b"\xffsolana offchain";
/// Application domain for generic (non-app-specific) off-chain signing.
const APPLICATION_DOMAIN: &[u8; 32] = &[0u8; 32];

const FORMAT_RESTRICTED_ASCII: u8 = 0;
const FORMAT_LIMITED_UTF8: u8 = 1;

/// Header byte count for v0 envelopes:
/// signing-domain (16) + version (1) + application_domain (32) + format (1)
/// + signers_count (1) + signer (32) + body_length (2 LE).
const V0_HEADER_LEN: usize = 16 + 1 + 32 + 1 + 1 + 32 + 2;

/// Body byte cap that fits in the Solana app's off-chain buffer.
/// `MAX_OFFCHAIN_MESSAGE_LENGTH` is 1232 (Solana wire MTU); subtract our
/// header length and round down.
const MAX_BODY_LEN: usize = 1232 - V0_HEADER_LEN;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: SubCmd,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

/// Sign or verify off-chain messages and files.
///
/// Signatures are produced over an off-chain message envelope in the format
/// the [LedgerHQ Solana app] parses: signing domain + v0 header
/// (application domain + format + signers + length) + body. Verifiers must
/// re-hash the envelope, not the original input.
///
/// `sign msg` puts the message text directly in the envelope body.
/// `sign file` hashes the file content and hex-encodes the digest as the
/// body — file size is unbounded, the on-the-wire envelope stays small,
/// and the device displays the hex hash for verification.
///
/// Format byte is auto-selected: printable ASCII → `RestrictedAscii`
/// (device shows the body directly), other UTF-8 ≤ ~1100 bytes →
/// `LimitedUtf8` (device shows hash; requires "Allow off-chain message
/// signing" enabled in the Solana app on the Ledger).
///
/// [LedgerHQ Solana app]: https://github.com/LedgerHQ/app-solana
#[derive(Debug, clap::Subcommand)]
pub enum SubCmd {
    File(FileCmd),
    Msg(MsgCmd),
    Verify(VerifyCmd),
}

impl SubCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::File(cmd) => cmd.run(opts).await,
            Self::Msg(cmd) => cmd.run(opts).await,
            Self::Verify(cmd) => cmd.run(opts).await,
        }
    }
}

/// Sign a file by its SHA-256.
///
/// The envelope body is `hex(SHA-256(file))` (64 ASCII chars). File size is
/// unbounded; the on-the-wire envelope stays small. Verifiers reproduce the
/// hash to confirm the signature describes the same file.
#[derive(Debug, clap::Args)]
pub struct FileCmd {
    /// Path to the file to sign.
    input: PathBuf,
}

impl FileCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        use std::io::Read;
        let mut content = Vec::new();
        fs::File::open(&self.input)?.read_to_end(&mut content)?;
        let body = hex_lower(&Sha256::digest(&content));
        sign_envelope(&opts, body.as_bytes(), BodyKind::FileSha256).await
    }
}

/// Sign a message string.
#[derive(Debug, clap::Args)]
pub struct MsgCmd {
    /// Message to sign.
    msg: String,
}

impl MsgCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        sign_envelope(&opts, self.msg.as_bytes(), BodyKind::Message).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum BodyKind {
    /// Body is the literal message bytes.
    Message,
    /// Body is the hex-encoded SHA-256 of a file's content (64 ASCII chars).
    FileSha256,
}

async fn sign_envelope(opts: &Opts, body: &[u8], body_kind: BodyKind) -> Result {
    if body.is_empty() {
        bail!("cannot sign an empty body");
    }
    if body.len() > MAX_BODY_LEN {
        bail!(
            "body too large: {} bytes (Ledger off-chain limit is {})",
            body.len(),
            MAX_BODY_LEN,
        );
    }

    let pubkey = opts.load_pubkey()?;
    let format = pick_format(body)?;
    let envelope = build_offchain_envelope(&pubkey, format, body);

    // The signing primitive forks by source. Ledger keypairs need
    // SIGN_OFFCHAIN_MESSAGE explicitly — `try_sign_message` would route
    // through SIGN_MESSAGE which the Solana app refuses for non-tx data.
    let signature: [u8; 64] = match opts.sources().first() {
        Some(WalletSource::Ledger { path, serial, .. }) => {
            let kp = helium_crypto::ledger::Keypair::from_derivation_path(
                helium_crypto::Network::MainNet,
                path.clone(),
                serial.as_deref(),
            )?;
            // For LimitedUtf8, the device displays a SHA-256 of the body
            // (post-header) on its blind-sign screen. Print the matching
            // hash on stderr so the user can compare. RestrictedAscii has
            // the body displayed verbatim — no hint needed.
            if format == FORMAT_LIMITED_UTF8 {
                let body_hash = Sha256::digest(body);
                eprintln!(
                    "→ Ledger off-chain blind-sign — verify hash on device: {}",
                    bs58::encode(body_hash).into_string()
                );
            }
            kp.sign_offchain_envelope(&envelope)?
        }
        _ => {
            let password = get_wallet_password(false)?;
            let kp = opts.load_keypair(password.as_bytes())?;
            let sig = kp.try_sign_message(&envelope)?;
            sig.as_ref()
                .try_into()
                .map_err(|_| anyhow!("unexpected ed25519 signature length"))?
        }
    };

    let helium = to_helium_pubkey(&pubkey)?;
    print_json(&json!({
        "address": {
            "solana": pubkey.to_string(),
            "helium": helium.to_string(),
        },
        "format": format_label(format),
        "body_kind": body_kind,
        "envelope": helium_lib::b64::encode(&envelope),
        "signature": bs58::encode(signature).into_string(),
    }))
}

/// Verify a previously-signed off-chain message.
///
/// The signer pubkey is extracted from the envelope. For file signatures,
/// pass `--file <path>` so the original file's hex SHA-256 is compared
/// against the envelope body.
#[derive(Debug, clap::Args)]
pub struct VerifyCmd {
    /// Base64 envelope from a previous sign invocation.
    #[arg(long)]
    envelope: String,
    /// Base58 signature.
    #[arg(long, short)]
    signature: String,
    /// Original file, for `sign file` signatures. Its hex SHA-256 is
    /// compared against the envelope body.
    #[arg(long)]
    file: Option<PathBuf>,
}

impl VerifyCmd {
    pub async fn run(&self, _opts: Opts) -> Result {
        use helium_crypto::Verify;

        let envelope = helium_lib::b64::decode(&self.envelope)?;
        let signature = bs58::decode(&self.signature)
            .into_vec()
            .map_err(|e| anyhow!("signature is not valid base58: {e}"))?;
        let parsed = parse_offchain_envelope(&envelope)?;

        let signer_helium = to_helium_pubkey(&parsed.signer)?;
        let sig_ok = signer_helium.verify(&envelope, &signature).is_ok();

        let mut json = json!({
            "address": {
                "solana": parsed.signer.to_string(),
                "helium": signer_helium.to_string(),
            },
            "format": format_label(parsed.format),
            "verified": sig_ok,
        });

        if let Some(path) = &self.file {
            use std::io::Read;
            let mut content = Vec::new();
            fs::File::open(path)?.read_to_end(&mut content)?;
            let expected = hex_lower(&Sha256::digest(&content));
            let body_str = std::str::from_utf8(parsed.body).unwrap_or("");
            let file_matches = body_str == expected;
            json["body_kind"] = json!(BodyKind::FileSha256);
            json["file_matches"] = json!(file_matches);
            json["verified"] = json!(sig_ok && file_matches);
        } else if let Ok(text) = std::str::from_utf8(parsed.body) {
            json["body_kind"] = json!(BodyKind::Message);
            json["body"] = json!(text);
        } else {
            json["body_kind"] = json!("opaque");
        }

        print_json(&json)
    }
}

/// Build a v0 off-chain message envelope in the layout the Solana Ledger
/// app's `parse_offchain_message_header` expects:
///
/// ```text
/// [signing_domain (16)]      \xffsolana offchain
/// [version       (1)]        0
/// [app_domain    (32)]       all zeros (generic)
/// [format        (1)]        0=RestrictedAscii, 1=LimitedUtf8
/// [signers_count (1)]        1
/// [signer        (32)]       caller's pubkey
/// [body_length   (2 LE)]
/// [body]
/// ```
fn build_offchain_envelope(signer: &Pubkey, format: u8, body: &[u8]) -> Vec<u8> {
    debug_assert!(
        body.len() <= u16::MAX as usize,
        "body length checked upstream"
    );
    let mut env = Vec::with_capacity(V0_HEADER_LEN + body.len());
    env.extend_from_slice(SIGNING_DOMAIN);
    env.push(0); // version
    env.extend_from_slice(APPLICATION_DOMAIN);
    env.push(format);
    env.push(1); // signers_count — firmware rejects 0
    env.extend_from_slice(signer.as_ref());
    env.extend_from_slice(&(body.len() as u16).to_le_bytes());
    env.extend_from_slice(body);
    env
}

struct ParsedEnvelope<'a> {
    format: u8,
    signer: Pubkey,
    body: &'a [u8],
}

fn parse_offchain_envelope(env: &[u8]) -> Result<ParsedEnvelope<'_>> {
    if env.len() < V0_HEADER_LEN {
        bail!("envelope too short ({} bytes)", env.len());
    }
    if &env[..16] != SIGNING_DOMAIN {
        bail!("envelope missing Solana off-chain signing domain prefix");
    }
    let mut cursor = 16;
    let version = env[cursor];
    cursor += 1;
    if version != 0 {
        bail!("unsupported envelope version {version}");
    }
    cursor += 32; // skip application_domain
    let format = env[cursor];
    cursor += 1;
    let signers_count = env[cursor];
    cursor += 1;
    if signers_count != 1 {
        bail!("envelope must declare exactly 1 signer (got {signers_count})");
    }
    let signer = Pubkey::try_from(&env[cursor..cursor + 32])
        .map_err(|_| anyhow!("invalid signer pubkey in envelope"))?;
    cursor += 32;
    let body_len = u16::from_le_bytes([env[cursor], env[cursor + 1]]) as usize;
    cursor += 2;
    if env.len() != cursor + body_len {
        bail!(
            "envelope body length mismatch: header claims {body_len} bytes, \
             {} actually present",
            env.len() - cursor,
        );
    }
    Ok(ParsedEnvelope {
        format,
        signer,
        body: &env[cursor..cursor + body_len],
    })
}

/// Pick the format byte the Solana app should clear-sign (`RestrictedAscii`)
/// vs blind-sign by hash (`LimitedUtf8`). Mirrors the heuristic in
/// `solana-offchain-message`.
fn pick_format(body: &[u8]) -> Result<u8> {
    if body.iter().all(|b| (0x20..=0x7e).contains(b)) {
        Ok(FORMAT_RESTRICTED_ASCII)
    } else if std::str::from_utf8(body).is_ok() {
        Ok(FORMAT_LIMITED_UTF8)
    } else {
        bail!("body must be valid UTF-8")
    }
}

fn format_label(format: u8) -> &'static str {
    match format {
        0 => "restricted_ascii",
        1 => "limited_utf8",
        _ => "unknown",
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use helium_lib::keypair::Keypair;

    #[test]
    fn envelope_layout_matches_v0_spec() {
        let pubkey = Pubkey::new_unique();
        let body = b"abc";
        let env = build_offchain_envelope(&pubkey, FORMAT_RESTRICTED_ASCII, body);
        // domain(16) + version(1) + app_domain(32) + format(1) + signers(1)
        // + signer(32) + length(2) + body
        assert_eq!(env.len(), V0_HEADER_LEN + 3);
        assert_eq!(&env[..16], SIGNING_DOMAIN);
        assert_eq!(env[16], 0); // version
        assert_eq!(&env[17..49], APPLICATION_DOMAIN); // app_domain zeroed
        assert_eq!(env[49], FORMAT_RESTRICTED_ASCII);
        assert_eq!(env[50], 1); // signers_count
        assert_eq!(&env[51..83], pubkey.as_ref());
        assert_eq!(&env[83..85], &3u16.to_le_bytes());
        assert_eq!(&env[85..], body);
    }

    #[test]
    fn parse_roundtrip() {
        let kp = Keypair::generate();
        let body = b"hello off-chain";
        let env = build_offchain_envelope(&kp.pubkey(), FORMAT_RESTRICTED_ASCII, body);

        let parsed = parse_offchain_envelope(&env).expect("parse");
        assert_eq!(parsed.format, FORMAT_RESTRICTED_ASCII);
        assert_eq!(parsed.signer, kp.pubkey());
        assert_eq!(parsed.body, body);
    }

    #[test]
    fn pick_format_ascii_then_utf8() {
        assert_eq!(pick_format(b"hello").unwrap(), FORMAT_RESTRICTED_ASCII);
        assert_eq!(
            pick_format("héllo".as_bytes()).unwrap(),
            FORMAT_LIMITED_UTF8
        );
    }

    #[test]
    fn file_hash_body_is_hex_ascii() {
        let pubkey = Pubkey::new_unique();
        let content = vec![0xab; 100_000];
        let body = hex_lower(&Sha256::digest(&content));
        assert_eq!(body.len(), 64);
        assert_eq!(
            pick_format(body.as_bytes()).unwrap(),
            FORMAT_RESTRICTED_ASCII
        );
        let env = build_offchain_envelope(&pubkey, FORMAT_RESTRICTED_ASCII, body.as_bytes());
        // Envelope stays small even though file is 100KB.
        assert_eq!(env.len(), V0_HEADER_LEN + 64);
    }

    #[test]
    fn software_sign_and_verify_roundtrip() {
        use helium_crypto::Verify;
        let kp = Keypair::generate();
        let body = b"verify me";
        let env = build_offchain_envelope(&kp.pubkey(), FORMAT_RESTRICTED_ASCII, body);

        let sig = kp.try_sign_message(&env).expect("sign");
        let parsed = parse_offchain_envelope(&env).expect("parse");
        let helium = to_helium_pubkey(&parsed.signer).expect("helium pk");
        assert!(helium.verify(&env, sig.as_ref()).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_body() {
        use helium_crypto::Verify;
        let kp = Keypair::generate();
        let env = build_offchain_envelope(&kp.pubkey(), FORMAT_RESTRICTED_ASCII, b"original");
        let sig = kp.try_sign_message(&env).expect("sign");

        let mut tampered = env.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;

        let helium = to_helium_pubkey(&kp.pubkey()).expect("helium pk");
        assert!(helium.verify(&tampered, sig.as_ref()).is_err());
    }

    #[test]
    fn parse_rejects_bad_domain() {
        let mut env = build_offchain_envelope(&Pubkey::new_unique(), 0, b"x");
        env[0] = 0x00;
        assert!(parse_offchain_envelope(&env).is_err());
    }

    #[test]
    fn parse_rejects_truncated() {
        let env = build_offchain_envelope(&Pubkey::new_unique(), 0, b"hello");
        let truncated = &env[..env.len() - 1];
        assert!(parse_offchain_envelope(truncated).is_err());
    }
}
