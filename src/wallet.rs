use crate::{
    format::{self, Format},
    keypair::{KeyTag, Keypair, PublicKey},
    mnemonic::mnemonic_to_entropy,
    pwhash::PwHash,
    result::{anyhow, bail, Result},
    traits::ReadWrite,
};
use aes_gcm::{
    aead::{generic_array::GenericArray, NewAead},
    AeadInPlace, Aes256Gcm,
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use sodiumoxide::randombytes;
use std::io::{self, Cursor};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

pub type Tag = [u8; 16];
pub type Iv = [u8; 12];
pub type AesKey = [u8; 32];

const WALLET_KIND_BASIC_V1: u16 = 0x0001;
const WALLET_KIND_BASIC_V2: u16 = 0x0002;

const WALLET_KIND_SHARDED_V1: u16 = 0x0101;
const WALLET_KIND_SHARDED_V2: u16 = 0x0102;

const PWHASH_KIND_PBKDF2: u8 = 0;
const PWHASH_KIND_ARGON2ID13: u8 = 1;

pub struct Wallet {
    pub public_key: PublicKey,
    pub iv: Iv,
    pub tag: Tag,
    pub encrypted: Vec<u8>,
    pub format: Format,
}

impl Wallet {
    /// Creates a basic wallet
    pub fn builder() -> Builder {
        Builder::default()
    }

    pub fn encrypt(keypair: &Keypair, password: &[u8], fmt: Format) -> Result<Wallet> {
        let mut encryption_key = AesKey::default();
        let mut format = fmt;
        let public_key = keypair.public_key();
        format.derive_key(password, &mut encryption_key)?;

        let mut iv = Iv::default();
        randombytes::randombytes_into(&mut iv);

        let aead = Aes256Gcm::new(GenericArray::from_slice(&encryption_key));

        let mut encrypted = vec![];
        keypair.write(&mut encrypted)?;

        match aead.encrypt_in_place_detached(
            iv.as_ref().into(),
            &public_key.to_vec(),
            &mut encrypted,
        ) {
            Err(_) => Err(anyhow!("Failed to encrypt wallet")),
            Ok(gtag) => Ok(Wallet {
                public_key: public_key.clone(),
                iv,
                tag: gtag.into(),
                encrypted,
                format,
            }),
        }
    }

    pub fn decrypt(&self, password: &[u8]) -> Result<Keypair> {
        let mut encryption_key = AesKey::default();
        let mut format = self.format.clone();
        format.derive_key(password, &mut encryption_key)?;

        let aead = Aes256Gcm::new(GenericArray::from_slice(&encryption_key));
        let mut buffer = self.encrypted.to_owned();
        match aead.decrypt_in_place_detached(
            self.iv.as_ref().into(),
            &self.public_key.to_vec(),
            &mut buffer,
            self.tag.as_ref().into(),
        ) {
            Err(_) => Err(anyhow!("Failed to decrypt wallet")),
            _ => Ok(()),
        }?;
        let keypair = Keypair::read(&mut Cursor::new(buffer))?;
        Ok(keypair)
    }

    pub fn address(&self) -> Result<String> {
        Ok(self.public_key.to_string())
    }

    pub fn pwhash(&self) -> &PwHash {
        self.format.pwhash()
    }

    fn mut_sharded_format(&mut self) -> Result<&mut format::Sharded> {
        match &mut self.format {
            Format::Sharded(format) => Ok(format),
            _ => Err(anyhow!("Wallet not sharded")),
        }
    }

    fn sharded_format(&self) -> Result<&format::Sharded> {
        match &self.format {
            Format::Sharded(format) => Ok(format),
            _ => Err(anyhow!("Wallet not sharded")),
        }
    }

    pub fn is_sharded(&self) -> bool {
        self.sharded_format().is_ok()
    }

    pub fn shards(&self) -> Result<Vec<Wallet>> {
        let format = self.sharded_format()?;
        let mut wallets = vec![];
        for shard in format.shards() {
            wallets.push(Self {
                format: Format::Sharded(shard),
                encrypted: self.encrypted.clone(),
                public_key: self.public_key.clone(),
                ..*self
            })
        }
        Ok(wallets)
    }

    pub fn absorb_shard(&mut self, shard: &Wallet) -> Result {
        let format = self.mut_sharded_format()?;
        let other_format = shard.sharded_format()?;

        format.absorb(other_format)
    }

    fn read_pwhash(reader: &mut dyn io::Read) -> Result<PwHash> {
        let kind = reader.read_u8()?;
        match kind {
            PWHASH_KIND_PBKDF2 => Ok(PwHash::pbkdf2_default()),
            PWHASH_KIND_ARGON2ID13 => Ok(PwHash::argon2id13_default()),
            _ => Err(anyhow!("Invalid pwhash kind {}", kind)),
        }
    }

    pub fn read(reader: &mut dyn io::Read) -> Result<Wallet> {
        let kind = reader.read_u16::<LittleEndian>()?;
        let mut format = match kind {
            WALLET_KIND_BASIC_V1 => Format::basic(PwHash::pbkdf2_default()),
            WALLET_KIND_BASIC_V2 => Format::basic(Self::read_pwhash(reader)?),
            WALLET_KIND_SHARDED_V1 => Format::sharded_default(PwHash::pbkdf2_default()),
            WALLET_KIND_SHARDED_V2 => Format::sharded_default(Self::read_pwhash(reader)?),
            _ => bail!("Invalid wallet kind {}", kind),
        };
        format.read(reader)?;
        let public_key = PublicKey::read(reader)?;
        let mut iv = Iv::default();
        reader.read_exact(&mut iv)?;
        format.mut_pwhash().read(reader)?;
        let mut tag = Tag::default();
        reader.read_exact(&mut tag)?;
        let mut encrypted = vec![];
        reader.read_to_end(&mut encrypted)?;

        Ok(Wallet {
            public_key,
            iv,
            tag,
            encrypted,
            format,
        })
    }

    fn write_pwhash(pwhash: &PwHash, writer: &mut dyn io::Write) -> Result {
        match pwhash {
            PwHash::Pbkdf2(_) => writer.write_u8(PWHASH_KIND_PBKDF2)?,
            PwHash::Argon2id13(_) => writer.write_u8(PWHASH_KIND_ARGON2ID13)?,
        }
        Ok(())
    }

    pub fn write(&self, writer: &mut dyn io::Write) -> Result {
        let kind = match self.format {
            Format::Basic(_) => WALLET_KIND_BASIC_V2,
            Format::Sharded(_) => WALLET_KIND_SHARDED_V2,
        };
        writer.write_u16::<LittleEndian>(kind)?;
        Self::write_pwhash(self.format.pwhash(), writer)?;
        self.format.write(writer)?;
        self.public_key.write(writer)?;
        writer.write_all(&self.iv)?;
        self.format.pwhash().write(writer)?;
        writer.write_all(&self.tag)?;
        writer.write_all(&self.encrypted)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ShardConfig {
    /// Number of shards to break the key into
    pub key_share_count: u8,

    /// Number of shards required to recover the key
    pub recovery_threshold: u8,
}

pub struct Builder {
    /// Output file to store the key in
    output: PathBuf,

    /// Password to access wallet
    password: String,

    pwhash: PwHash,

    /// Overwrite an existing file
    force: bool,

    /// The seed words used to create this wallet
    seed_words: Option<Vec<String>>,

    /// The KeyTag (network and key type) to use for this wallet
    key_tag: Option<KeyTag>,

    /// Optional shard config info to use in order to create a sharded wallet
    /// otherwise, creates a basic non-sharded wallet
    shard: Option<ShardConfig>,

    /// Optional swarm file to import instead of generating from scratch.
    swarm_input: Option<PathBuf>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder {
            output: PathBuf::from("wallet.key"),
            password: Default::default(),
            pwhash: PwHash::argon2id13_default(),
            force: false,
            seed_words: None,
            key_tag: None,
            shard: None,
            swarm_input: None,
        }
    }

    /// Sets the output file for the wallet.
    /// Defaults to 'wallet.key'
    pub fn output(mut self, path: &Path) -> Builder {
        self.output = path.to_path_buf();
        self
    }

    /// Sets the wallet's password
    /// Defaults to '' (empty string)
    pub fn password(mut self, pwd: &str) -> Builder {
        self.password = pwd.to_owned();
        self
    }

    /// Sets the wallet's password hasher
    /// Defaults to `PwHash::argon2id13_default()`
    pub fn pwhash(mut self, pwhash: PwHash) -> Builder {
        self.pwhash = pwhash.to_owned();
        self
    }

    /// Force overwrite of wallet if output already exists
    /// Defaults to false
    pub fn force(mut self, overwrite: bool) -> Builder {
        self.force = overwrite;
        self
    }

    /// The seed words used to create this wallet
    /// Defaults to None
    pub fn seed_words(mut self, seed_words: Option<Vec<String>>) -> Builder {
        self.seed_words = seed_words;
        self
    }

    /// The type of key to generate (ecc_compact/ed25519)
    /// Defaults to ed25519
    pub fn key_tag(mut self, key_tag: &KeyTag) -> Builder {
        self.key_tag = Some(*key_tag);
        self
    }

    /// Optional shard config info to use in order to create a sharded wallet
    /// otherwise, creates a basic non-sharded wallet
    pub fn shard(mut self, shard_config: Option<ShardConfig>) -> Builder {
        self.shard = shard_config;
        self
    }

    /// Load an Erlang-based node swarm key.
    pub fn from_swarm(mut self, path: PathBuf) -> Builder {
        self.swarm_input = Some(path);
        self
    }

    /// Creates a new wallet
    pub fn create(self) -> Result<Wallet> {
        let keypair = match (self.swarm_input, self.key_tag) {
            (Some(swarm_input_path), None) =>
                load_keypair_from_swarm(&swarm_input_path)?,
            (None, key_tag_opt) =>
                gen_keypair(key_tag_opt.unwrap_or_default(), self.seed_words)?,
            (Some(_), _) => return Err(anyhow!("Can't import from swarm key and also set key type, network, or seed words at the same time."))
        };

        let wallet = if let Some(shard_config) = &self.shard {
            let format = format::Sharded {
                key_share_count: shard_config.key_share_count,
                recovery_threshold: shard_config.recovery_threshold,
                pwhash: self.pwhash,
                key_shares: vec![],
            };
            Wallet::encrypt(&keypair, self.password.as_bytes(), Format::Sharded(format))?
        } else {
            let format = format::Basic {
                pwhash: PwHash::argon2id13_default(),
            };
            Wallet::encrypt(&keypair, self.password.as_bytes(), Format::Basic(format))?
        };

        if self.shard.is_some() {
            let extension = self
                .output
                .extension()
                .unwrap_or_else(|| OsStr::new(""))
                .to_str()
                .unwrap()
                .to_string();
            for (i, shard) in wallet.shards()?.iter().enumerate() {
                let mut filename = self.output.clone();
                let share_extension = format!("{}.{}", extension, (i + 1));
                filename.set_extension(share_extension);
                let mut writer = open_output_file(&filename, !self.force)?;
                shard.write(&mut writer)?;
            }
        } else {
            let mut writer = open_output_file(&self.output, !self.force)?;
            wallet.write(&mut writer)?;
        }

        Ok(wallet)
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

fn gen_keypair(tag: KeyTag, seed_words: Option<Vec<String>>) -> Result<Keypair> {
    // Callers of this function should either have Some of both or None of both.
    // Anything else is an error.
    match seed_words {
        Some(words) => {
            let entropy = mnemonic_to_entropy(words)?;
            Keypair::generate_from_entropy(tag, &entropy)
        }
        None => Ok(Keypair::generate(tag)),
    }
}

fn load_keypair_from_swarm(filename: &Path) -> Result<Keypair> {
    let mut stream = fs::OpenOptions::new().read(true).open(filename)?;
    Keypair::read(&mut stream)
}

fn open_output_file(filename: &Path, create: bool) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .create_new(create)
        .open(filename)
}

//
// Test
//

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rountrip_basic() {
        let from_keypair = Keypair::default();
        let format = format::Basic {
            pwhash: PwHash::argon2id13_default(),
        };
        let password = b"passsword";
        let wallet = Wallet::encrypt(&from_keypair, password, Format::Basic(format))
            .expect("wallet creation");
        let to_keypair = wallet.decrypt(password).expect("wallet to keypair");
        assert_eq!(from_keypair, to_keypair);
    }

    #[test]
    fn basic_from_builder() {
        use std::fs;
        let path: PathBuf = ".test-basic.key".into();
        // Delete Any existing test wallet in case prev error
        let _ = fs::remove_file(&path);

        let password = String::from("password");
        let tag = KeyTag::default();
        let seed_words: Vec<String> = vec![
            "drill".to_string(),
            "toddler".to_string(),
            "tongue".to_string(),
            "laundry".to_string(),
            "access".to_string(),
            "silly".to_string(),
            "few".to_string(),
            "faint".to_string(),
            "glove".to_string(),
            "birth".to_string(),
            "crumble".to_string(),
            "add".to_string(),
        ];
        let from_keypair =
            gen_keypair(tag, Some(seed_words.clone())).expect("to generate a keypair");

        let wallet = Wallet::builder()
            .password(&password)
            .output(&path)
            .key_tag(&tag)
            .seed_words(Some(seed_words.clone()))
            .create()
            .expect("wallet to be created");

        let to_keypair = wallet
            .decrypt(password.as_bytes())
            .expect("wallet to keypair");
        assert_eq!(from_keypair, to_keypair);

        // clean up
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sharded_from_builder() {
        let path = Path::new(".test-sharded.key");
        // Delete Any existing test wallet in case prev error
        let _ = clean_up_shards(&path, 3);

        let password = String::from("password");
        let tag = KeyTag::default();
        let shard_config = ShardConfig {
            key_share_count: 3,
            recovery_threshold: 2,
        };

        let seed_words: Vec<String> = vec![
            "drill".to_string(),
            "toddler".to_string(),
            "tongue".to_string(),
            "laundry".to_string(),
            "access".to_string(),
            "silly".to_string(),
            "few".to_string(),
            "faint".to_string(),
            "glove".to_string(),
            "birth".to_string(),
            "crumble".to_string(),
            "add".to_string(),
        ];
        let from_keypair =
            gen_keypair(tag, Some(seed_words.clone())).expect("to generate a keypair");

        let wallet = Wallet::builder()
            .password(&password)
            .output(&path)
            .key_tag(&tag)
            .seed_words(Some(seed_words.clone()))
            .shard(Some(shard_config))
            .create()
            .expect("wallet to be created");

        let to_keypair = wallet
            .decrypt(password.as_bytes())
            .expect("wallet to keypair");
        assert_eq!(from_keypair, to_keypair);

        // clean up
        let _ = clean_up_shards(&path, 3);
    }

    fn clean_up_shards(path: &Path, shards: u8) {
        for i in 1..=shards {
            let _ = fs::remove_file(format!("{}.{}", path.to_string_lossy(), i));
        }
    }

    #[test]
    fn rountrip_sharded() {
        let from_keypair = Keypair::default();
        let format = format::Sharded {
            key_share_count: 5,
            recovery_threshold: 3,
            pwhash: PwHash::argon2id13_default(),
            key_shares: vec![],
        };
        let password = b"passsword";
        let wallet = Wallet::encrypt(&from_keypair, password, Format::Sharded(format))
            .expect("wallet creation");
        let to_keypair = wallet.decrypt(password).expect("wallet to keypair");
        assert_eq!(from_keypair, to_keypair);
    }

    #[test]
    fn keypair_from_ecc_compact_swarm() {
        use crate::keypair::{KeyType, Network};

        let mut swarm_key: &[u8] = &[
            0x00, // <- Type/Network: ecc_compact/mainnet
            // Private exponent
            0xd3, 0x98, 0xd1, 0xfc, 0x3f, 0x4c, 0x22, 0x79, 0xa8, 0x9a, 0x25, 0xb2, 0xd3, 0x52,
            0xcf, 0x38, 0x25, 0xcf, 0xc0, 0x18, 0x8d, 0xd7, 0xee, 0xb3, 0x04, 0x7c, 0x29, 0x29,
            0x0b, 0xdf, 0xaa, 0xd2,
            // End private exponent
            // Begin public key
            0x04, // <- ECC Point tag: Full X/Y pair
            // Begin X-coordinate
            0x33, 0x4b, 0xb9, 0x5a, 0x68, 0x15, 0x4a, 0x74, 0xed, 0xf6, 0x51, 0x81, 0x6b, 0xd5,
            0x56, 0x11, 0xb5, 0xba, 0x85, 0x04, 0x3f, 0xbd, 0x20, 0xa3, 0x74, 0xa3, 0x83, 0x1b,
            0x83, 0x98, 0x04, 0x35,
            // End X-coordinate
            // Begin Y-coordinate
            0x2e, 0x7d, 0xcd, 0x51, 0xa1, 0x90, 0x92, 0x63, 0x77, 0x9e, 0x4d, 0x0c, 0x6d, 0xb5,
            0x2e, 0x23, 0x2c, 0x9b, 0xdb, 0x64, 0xe1, 0x94, 0x24, 0x08, 0x99, 0x1c, 0x4b, 0x29,
            0xac, 0xd7, 0x6c, 0xce,
            // End Y-coordinate
            // End public key
        ];

        let seed_words = [
            "squeeze", "shoot", "lecture", "leader", "season", "devote", "pen", "dwarf", "ready",
            "once", "record", "icon", "friend", "theme", "giraffe", "road", "upgrade", "oblige",
            "business", "false", "mouse", "used", "prize", "foster",
        ];

        let tag = KeyTag {
            key_type: KeyType::EccCompact,
            network: Network::MainNet,
        };

        swarm_keypair_test(&mut swarm_key, seed_words, tag)
    }

    #[test]
    fn keypair_from_ed25519_testnet_swarm() {
        use crate::keypair::{KeyType, Network};

        let mut swarm_key: &[u8] = &[
            0x11, // <- Type/Network: ed25519/testnet
            // Private key
            0xE6, 0xAB, 0x3A, 0x58, 0x3C, 0x6F, 0x7B, 0xDE, 0x59, 0x9B, 0xC9, 0x07, 0x2E, 0xE6,
            0xA2, 0xED, 0xA4, 0xFC, 0xF0, 0x81, 0x0E, 0xC7, 0x26, 0x9B, 0x98, 0xC8, 0xD3, 0x6A,
            0xBA, 0x67, 0xF5, 0x81, 0x04, 0xF1, 0x88, 0x52, 0x62, 0x64, 0x3F, 0x12, 0xB7, 0x5F,
            0x75, 0x3B, 0x0F, 0x6A, 0xD9, 0xB5, 0x83, 0xCE, 0xE0, 0x50, 0xC9, 0xA1, 0xEE, 0xBA,
            0x20, 0x14, 0x63, 0xF0, 0x3B, 0xB1, 0xFE, 0x13,
            // End private key
            // Begin public key
            0x04, 0xF1, 0x88, 0x52, 0x62, 0x64, 0x3F, 0x12, 0xB7, 0x5F, 0x75, 0x3B, 0x0F, 0x6A,
            0xD9, 0xB5, 0x83, 0xCE, 0xE0, 0x50, 0xC9, 0xA1, 0xEE, 0xBA, 0x20, 0x14, 0x63, 0xF0,
            0x3B, 0xB1, 0xFE, 0x13,
            // End public key
        ];

        let seed_words = [
            "trade", "flush", "noodle", "juice", "waste", "upset", "grid", "junior", "already",
            "jaguar", "post", "swap", "exist", "joke", "aerobic", "suggest", "charge", "system",
            "cram", "plug", "produce", "crop", "stock", "couple",
        ];

        let tag = KeyTag {
            key_type: KeyType::Ed25519,
            network: Network::TestNet,
        };

        swarm_keypair_test(&mut swarm_key, seed_words, tag)
    }

    fn swarm_keypair_test(swarm_key_bytes: &mut &[u8], seed_words: [&str; 24], key_type: KeyTag) {
        let seed_words_vec = seed_words.iter().map(|&s| s.to_owned()).collect();
        let from_swarm = Keypair::read(swarm_key_bytes).unwrap();
        let from_seed = gen_keypair(key_type, Some(seed_words_vec)).unwrap();
        assert_eq!(from_seed, from_swarm);
    }
}
