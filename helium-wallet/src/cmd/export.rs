use crate::{cmd::*, pwhash::*};
use helium_lib::keypair::PublicKey;
use qr2term::print_qr;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sodiumoxide::crypto::{pwhash::argon2id13 as pwhash, secretbox::xsalsa20poly1305 as secretbox};

//NOTE: The ops and memlimits are set lower than the CLI wallet uses for itself because
//      initial testing on the mobile devices found SENSITIVE settings took too long.
const ARGON_OPS_LIMIT: pwhash::OpsLimit = pwhash::OPSLIMIT_MODERATE;
const ARGON_MEM_LIMIT: pwhash::MemLimit = pwhash::MEMLIMIT_MODERATE;

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum OutputFormat {
    /// Export the raw secret key, compatible with the solana CLI wallet
    Key,
    /// Export the seed phrase
    Seed,
    /// Export the encrypted seed via QR-encoded JSON.    
    Qr,
}

/// Exports encrypted wallet seed as QR-encoded JSON or raw seed via stdout.
#[derive(Debug, clap::Args)]
pub struct Cmd {
    /// Output format to use
    #[arg(long, default_value = "seed")]
    output: OutputFormat,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EncryptedSeed {
    version: u16,
    salt: String,
    nonce: String,
    ciphertext: String,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = opts.load_wallet()?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        match self.output {
            OutputFormat::Qr => {
                let seed_pwd = get_password("Export Password", true)?;
                let json = json!({
                    "address": wallet.public_key.to_string(),
                    "seed": encrypt_seed_v1(&keypair, &seed_pwd)?,
                });
                print_qr(json.to_string())?;
                Ok(())
            }
            OutputFormat::Key => {
                println!("{}", &serde_json::to_string(&keypair.secret())?);
                Ok(())
            }
            OutputFormat::Seed => {
                println!("{}", &keypair.phrase()?);
                Ok(())
            }
        }
    }
}

/// Encrypted seeds V1:
///  1) Given the user entered password, generate an encryption key using the same pwhash
///     algorithm (Argong2id13) as the existing wallet.
///  2) Use libsodium xsalsa20poly1305 and the encryption key to encrypt the seed phrase.
///  3) base64 encode the salt, the nonce, and the encrypted result so it is easier to
///     render in JSON later.
pub fn encrypt_seed_v1(keypair: &Keypair, password: &String) -> Result<EncryptedSeed> {
    let address = keypair.public_key().to_string();
    let phrase = keypair.phrase()?;

    let hasher = Argon2id13::with_limits(ARGON_OPS_LIMIT, ARGON_MEM_LIMIT);
    let mut key = secretbox::Key([0; secretbox::KEYBYTES]);
    let secretbox::Key(ref mut key_buffer) = key;
    hasher.pwhash(password.as_bytes(), key_buffer)?;

    let nonce = secretbox::gen_nonce();
    let ciphertext = secretbox::seal(phrase.as_bytes(), &nonce, &key);

    let result = EncryptedSeed {
        version: 1,
        salt: b64::encode(hasher.salt()),
        nonce: b64::encode(nonce),
        ciphertext: b64::encode(ciphertext),
    };

    if cfg!(debug_assertions) {
        println!("DEBUG encrypt_seed_v1:  password: {password}");
        println!("DEBUG encrypt_seed_v1:  key: {}", b64::encode(key));
        let json_data = json!({
            "address": address,
            "seed": result,
        });
        print_json(&json_data)?;
    };

    Ok(result)
}

/// Decrypt an EncryptedSeed that was encrypted by encrypt_seed_v1
///
pub fn decrypt_seed_v1(es: &EncryptedSeed, password: &String) -> Result<String> {
    if es.version != 1 {
        bail!("Incompatible version format");
    }
    let salt = pwhash::Salt::from_slice(b64::decode(&es.salt)?.as_slice())
        .ok_or_else(|| anyhow::anyhow!("Failed to decode salt"))?;
    let hasher = Argon2id13::with_salt_and_limits(salt, ARGON_OPS_LIMIT, ARGON_MEM_LIMIT);
    let mut key = secretbox::Key([0; secretbox::KEYBYTES]);
    let secretbox::Key(ref mut key_buffer) = key;
    hasher.pwhash(password.as_bytes(), key_buffer)?;

    let nonce: [u8; secretbox::NONCEBYTES] = b64::decode(&es.nonce)?.as_slice().try_into()?;
    let ciphertext = b64::decode(&es.ciphertext)?;

    if cfg!(debug_assertions) {
        println!("DEBUG decrypt_seed_v1: password: {password}");
        println!("DEBUG decrypt_seed_v1: es: {es:?}");
        println!("DEBUG decrypt_seed_v1: nonce: {nonce:?}, salt: {salt:?}");
    };

    if let Ok(decrypted_bytes) = secretbox::open(&ciphertext, &secretbox::Nonce(nonce), &key) {
        String::from_utf8(decrypted_bytes).map_err(anyhow::Error::from)
    } else {
        Err(anyhow::anyhow!("Couldn't decrypt EncryptedSeed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON_DATA: &str = r#"
        {
            "ciphertext": "yKU6haopJpIjIWoiYxa07fGnXjtgh30zuOv9PKQcs59tlOqrjUCSqFITr7wi2ARkypZQZ2BnM4UsjcNGU7oBBHUg4MTqhiWrKyFXDs6AdjOL5RzZasB2cvtA4a/35znyG7E2m+aydn9gUKCpS60fQdcDS7cjO6BilGH82PUof3NcnvmSs6pr526b+ooqCexPpXrR0oc+9gpjGjndekxzXfJ+Wk6tdfRT/r74",
            "nonce": "n1RCu1R3tDt6UDQ4Zv8bMPRMvWA9BcKM",
            "salt": "w9NIcC6BBrTuxqkXBursmw==",
            "version": 1
        }"#;

    const MNEMONIC_PHRASE: &str = "pelican sphere tackle click broken hurt \
                                    fork nephew choice seven announce moment \
                                    tobacco tribe topple pause october drama \
                                    sock erase news glove okay bubble";
    const SEED_PWD: &str = "h3l1Um";

    fn create_test_keypair() -> Keypair {
        let entropy = helium_mnemonic::mnemonic_to_entropy(phrase_to_words(MNEMONIC_PHRASE))
            .expect("mnemonic");
        Keypair::generate_from_entropy(&entropy).expect("keypair")
    }

    #[test]
    fn decrypt_seed() {
        let es: EncryptedSeed = serde_json::from_str(JSON_DATA).expect("Failed to parse JSON");
        let decrypted_phrase =
            decrypt_seed_v1(&es, &String::from(SEED_PWD)).expect("Failed to decrypt");

        assert_eq!(decrypted_phrase, String::from(MNEMONIC_PHRASE));
    }

    #[test]
    fn decrypt_seed_fail() {
        let es: EncryptedSeed = serde_json::from_str(JSON_DATA).expect("Failed to parse JSON");
        decrypt_seed_v1(&es, &String::from("fizbuzz"))
            .expect_err("Should not been able to decrypt");
    }

    #[test]
    fn encrypt_decrypt_seed() {
        let keypair = create_test_keypair();
        let created_es: EncryptedSeed = encrypt_seed_v1(&keypair, &String::from(SEED_PWD))
            .expect("Failed to encrypt seed phrase");

        let decrypted_phrase = decrypt_seed_v1(&created_es, &String::from(SEED_PWD))
            .expect("Failed to decrypt seed phrase");
        assert_eq!(decrypted_phrase, MNEMONIC_PHRASE);
    }
}
