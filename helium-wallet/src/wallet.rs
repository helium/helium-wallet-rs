use crate::{
    format::{self, Format},
    pwhash::PwHash,
    read_write::ReadWrite,
    result::{anyhow, bail, Error, Result},
};
use aes_gcm::{aead::generic_array::GenericArray, AeadInPlace, Aes256Gcm, KeyInit};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use helium_lib::keypair::{to_helium_pubkey, Keypair, Pubkey, PublicKey, PUBKEY_BYTES};
use sodiumoxide::randombytes;
use std::io::{self, Cursor};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub type Tag = [u8; 16];
pub type Iv = [u8; 12];
pub type AesKey = [u8; 32];

const WALLET_KIND_BASIC_V1: u16 = 0x0001;
const WALLET_KIND_BASIC_V2: u16 = 0x0002;
const WALLET_KIND_BASIC_V3: u16 = 0x0003;

const WALLET_KIND_SHARDED_V1: u16 = 0x0101;
const WALLET_KIND_SHARDED_V2: u16 = 0x0102;
const WALLET_KIND_SHARDED_V3: u16 = 0x0103;

const PWHASH_KIND_PBKDF2: u8 = 0;
const PWHASH_KIND_ARGON2ID13: u8 = 1;

pub struct Wallet {
    pub public_key: Pubkey,
    pub iv: Iv,
    pub tag: Tag,
    pub encrypted: Vec<u8>,
    pub format: Format,
    pub kind: u16,
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
        let kind = Self::format_to_kind(&format);

        match aead.encrypt_in_place_detached(
            iv.as_ref().into(),
            &public_key.to_bytes(),
            &mut encrypted,
        ) {
            Err(_) => Err(anyhow!("Failed to encrypt wallet")),
            Ok(gtag) => Ok(Wallet {
                public_key,
                iv,
                tag: gtag.into(),
                encrypted,
                format,
                kind,
            }),
        }
    }

    pub fn decrypt(&self, password: &[u8]) -> Result<Arc<Keypair>> {
        let mut encryption_key = AesKey::default();
        let mut format = self.format.clone();
        format.derive_key(password, &mut encryption_key)?;

        let aead = Aes256Gcm::new(GenericArray::from_slice(&encryption_key));
        let pubkey_bytes: Vec<u8> = match self.kind {
            WALLET_KIND_BASIC_V1
            | WALLET_KIND_BASIC_V2
            | WALLET_KIND_SHARDED_V1
            | WALLET_KIND_SHARDED_V2 => {
                let mut bytes = vec![0; PUBKEY_BYTES + 1];
                bytes[0] = helium_crypto::KeyTag {
                    network: helium_crypto::Network::MainNet,
                    key_type: helium_crypto::KeyType::Ed25519,
                }
                .into();
                bytes[1..].copy_from_slice(&self.public_key.to_bytes());
                bytes
            }
            WALLET_KIND_BASIC_V3 | WALLET_KIND_SHARDED_V3 => self.public_key.to_bytes().to_vec(),
            _ => unreachable!(),
        };

        let mut buffer = self.encrypted.to_owned();
        if aead
            .decrypt_in_place_detached(
                self.iv.as_ref().into(),
                &pubkey_bytes,
                &mut buffer,
                self.tag.as_ref().into(),
            )
            .is_err()
        {
            bail!("Failed to decrypt wallet");
        }
        let keypair = Self::read_keypair(&mut Cursor::new(buffer), self.kind)?;
        Ok(Arc::new(keypair))
    }

    pub fn address(&self) -> Result<String> {
        Ok(self.public_key.to_string())
    }

    pub fn helium_address(&self) -> Result<String> {
        self.helium_pubkey().map(|v| v.to_string())
    }

    pub fn helium_pubkey(&self) -> Result<helium_crypto::PublicKey> {
        Ok(to_helium_pubkey(&self.public_key)?)
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
                public_key: self.public_key,
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

    fn read_pubkey(reader: &mut dyn io::Read, kind: u16) -> Result<Pubkey> {
        match kind {
            WALLET_KIND_BASIC_V1
            | WALLET_KIND_BASIC_V2
            | WALLET_KIND_SHARDED_V1
            | WALLET_KIND_SHARDED_V2 => {
                let helium_pubkey = helium_crypto::PublicKey::read(reader)?;
                Pubkey::try_from(helium_pubkey).map_err(Error::from)
            }
            WALLET_KIND_BASIC_V3 | WALLET_KIND_SHARDED_V3 => Pubkey::read(reader),
            _ => bail!("Invalid wallet kind {kind}"),
        }
    }

    fn read_keypair(reader: &mut dyn io::Read, kind: u16) -> Result<Keypair> {
        use helium_crypto::KeyType;
        match kind {
            WALLET_KIND_BASIC_V1
            | WALLET_KIND_BASIC_V2
            | WALLET_KIND_SHARDED_V1
            | WALLET_KIND_SHARDED_V2 => {
                let tag = reader.read_u8()?;
                match KeyType::try_from(tag)? {
                    KeyType::Ed25519 => Keypair::read(reader),
                    _ => bail!("Unsupported key type: {tag}"),
                }
            }
            WALLET_KIND_BASIC_V3 | WALLET_KIND_SHARDED_V3 => Keypair::read(reader),
            _ => bail!("Invalid wallet kind {kind}"),
        }
    }

    fn format_to_kind(format: &Format) -> u16 {
        match format {
            Format::Basic(_) => WALLET_KIND_BASIC_V3,
            Format::Sharded(_) => WALLET_KIND_SHARDED_V3,
        }
    }

    pub fn read(reader: &mut dyn io::Read) -> Result<Wallet> {
        let kind = reader.read_u16::<LittleEndian>()?;
        let mut format = match kind {
            WALLET_KIND_BASIC_V1 => Format::basic(PwHash::pbkdf2_default()),
            WALLET_KIND_BASIC_V2 | WALLET_KIND_BASIC_V3 => {
                Format::basic(Self::read_pwhash(reader)?)
            }
            WALLET_KIND_SHARDED_V1 => Format::sharded_default(PwHash::pbkdf2_default()),
            WALLET_KIND_SHARDED_V2 | WALLET_KIND_SHARDED_V3 => {
                Format::sharded_default(Self::read_pwhash(reader)?)
            }
            _ => bail!("Invalid wallet kind {kind}"),
        };
        format.read(reader)?;
        let public_key = Self::read_pubkey(reader, kind)?;
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
            kind,
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
        let kind = Self::format_to_kind(&self.format);
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

    /// The seed phrase used to create this wallet
    seed_phrase: Option<Vec<String>>,

    /// Optional shard config info to use in order to create a sharded wallet
    /// otherwise, creates a basic non-sharded wallet
    shard: Option<ShardConfig>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder {
            output: PathBuf::from("wallet.key"),
            password: Default::default(),
            pwhash: PwHash::argon2id13_default(),
            force: false,
            seed_phrase: None,
            shard: None,
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
        pwd.clone_into(&mut self.password);
        self
    }

    /// Sets the wallet's password hasher
    /// Defaults to `PwHash::argon2id13_default()`
    pub fn pwhash(mut self, pwhash: PwHash) -> Builder {
        pwhash.clone_into(&mut self.pwhash);
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
    pub fn seed_phrase(mut self, seed_phrase: Option<Vec<String>>) -> Builder {
        self.seed_phrase = seed_phrase;
        self
    }

    /// Optional shard config info to use in order to create a sharded wallet
    /// otherwise, creates a basic non-sharded wallet
    pub fn shard(mut self, shard_config: Option<ShardConfig>) -> Builder {
        self.shard = shard_config;
        self
    }

    /// Creates a new wallet
    pub fn create(self) -> Result<Wallet> {
        let keypair = gen_keypair(self.seed_phrase)?;

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

fn gen_keypair(seed_words: Option<Vec<String>>) -> Result<Arc<Keypair>> {
    // Callers of this function should either have Some of both or None of both.
    // Anything else is an error.
    match seed_words {
        Some(words) => Ok(Keypair::from_words(words)?),
        None => Ok(Keypair::generate().into()),
    }
}

fn open_output_file(filename: &Path, create: bool) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .create_new(create)
        .open(filename)
}

//
// Test
//

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::phrase_to_words;

    #[test]
    fn rountrip_basic() {
        let from_keypair: Arc<Keypair> = Keypair::default().into();
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
        let seed_words = phrase_to_words(
            "drill toddler tongue laundry access silly few faint glove birth crumble add",
        );

        let from_keypair = gen_keypair(Some(seed_words.clone())).expect("to generate a keypair");

        let wallet = Wallet::builder()
            .password(&password)
            .output(&path)
            .seed_phrase(Some(seed_words.clone()))
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
        let shard_config = ShardConfig {
            key_share_count: 3,
            recovery_threshold: 2,
        };

        let seed_words = phrase_to_words(
            "moment case dirt ski tool dynamic sort ugly pluck drop kiwi knee jar easy verb canal nuclear survey before dwarf prosper cave pottery target");
        let from_keypair = gen_keypair(Some(seed_words.clone())).expect("to generate a keypair");

        let wallet = Wallet::builder()
            .password(&password)
            .output(&path)
            .seed_phrase(Some(seed_words.clone()))
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
        let from_keypair = Arc::new(Keypair::default());
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
}
