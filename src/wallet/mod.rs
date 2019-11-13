use crate::{
    keypair::{self, Keypair, PubKeyBin},
    result::Result,
    traits::{ReadWrite, B58},
};
use aead::NewAead;
use aes_gcm::Aes256Gcm;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use hmac::{Hmac, Mac};
use pbkdf2;
use sha2::Sha256;
use shamirsecretsharing::hazmat::{combine_keyshares, create_keyshares};
use sodiumoxide::randombytes;
use std::{
    boxed::Box,
    fmt,
    io::{self, Cursor},
};

pub type Salt = [u8; 8];
pub type Tag = [u8; 16];
pub type IV = [u8; 12];
pub type AESKey = [u8; 32];
pub type SSSKey = [u8; 32];

#[derive(Clone)]
pub struct KeyShare(pub(crate) [u8; 33]);

type HmacSha256 = Hmac<Sha256>;

const WALLET_KIND_BASIC: u16 = 0x0001;
const WALLET_KIND_SHARDED: u16 = 0x0101;
const WALLET_DEFAULT_ITERATIONS: u32 = 1_000_000;

impl Default for KeyShare {
    fn default() -> Self {
        KeyShare([0; 33])
    }
}

impl KeyShare {
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }

    pub fn from_slice(slice: &[u8]) -> KeyShare {
        let mut share = [0u8; 33];
        share.copy_from_slice(slice);
        KeyShare(share)
    }
}

impl fmt::Debug for KeyShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyShare({:?})", self.0.to_vec())
    }
}

pub struct Wallet {
    pub pubkey_bin: PubKeyBin,
    pub iterations: u32,
    pub iv: IV,
    pub salt: Salt,
    pub tag: Tag,
    pub encrypted: Vec<u8>,
    pub format: Box<dyn WalletFormat>,
}

pub trait WalletFormat {
    fn wallet_for_keypair(
        &mut self,
        keypair: &Keypair,
        password: &[u8],
        iterations: u32,
    ) -> Result<Wallet>;
    fn keypair_for_wallet(&self, wallet: &Wallet, password: &[u8]) -> Result<Keypair>;
    fn as_sharded_format(&self) -> Result<ShardedFormat>;
    fn absorb_key_shares(&mut self, other: &ShardedFormat) -> Result;
    fn read_wallet(&self, reader: &mut dyn io::Read) -> Result<Wallet>;
    fn write_wallet(&self, wallet: &Wallet, writer: &mut dyn io::Write) -> Result;
}

#[derive(Clone, Debug, Default)]
pub struct BasicFormat {}

#[derive(Clone, Debug)]
pub struct ShardedFormat {
    pub key_share_count: u8,
    pub recovery_threshold: u8,
    pub key_shares: Vec<KeyShare>,
}

impl Default for ShardedFormat {
    fn default() -> Self {
        Self {
            key_share_count: 5,
            recovery_threshold: 3,
            key_shares: Vec::new(),
        }
    }
}

impl WalletFormat for BasicFormat {
    fn wallet_for_keypair(
        &mut self,
        keypair: &Keypair,
        password: &[u8],
        iterations: u32,
    ) -> Result<Wallet> {
        let mut salt = Salt::default();
        let mut encryption_key = AESKey::default();
        stretch_password(password, iterations, &mut salt, &mut encryption_key)?;

        let mut wallet = Wallet {
            salt,
            iterations,
            pubkey_bin: PubKeyBin::default(),
            encrypted: Vec::new(),
            iv: IV::default(),
            tag: Tag::default(),
            format: Box::new(self.to_owned()),
        };
        encrypt_keypair(
            keypair,
            &encryption_key,
            &mut wallet.iv,
            &mut wallet.pubkey_bin,
            &mut wallet.encrypted,
            &mut wallet.tag,
        )?;
        Ok(wallet)
    }

    fn keypair_for_wallet(&self, wallet: &Wallet, password: &[u8]) -> Result<Keypair> {
        let mut encryption_key = AESKey::default();
        re_stretch_password(
            password,
            wallet.iterations,
            wallet.salt,
            &mut encryption_key,
        )?;
        let keypair = decrypt_keypair(
            &wallet.encrypted,
            &encryption_key,
            &wallet.pubkey_bin,
            &wallet.iv,
            &wallet.tag,
        )?;
        Ok(keypair)
    }

    fn as_sharded_format(&self) -> Result<ShardedFormat> {
        Err("Not a sharded wallet".into())
    }

    fn absorb_key_shares(&mut self, _other: &ShardedFormat) -> Result {
        Err("Basic wallet has no key shares".into())
    }

    fn read_wallet(&self, reader: &mut dyn io::Read) -> Result<Wallet> {
        let mut wallet = Wallet {
            salt: Salt::default(),
            pubkey_bin: PubKeyBin::read(reader)?,
            iterations: WALLET_DEFAULT_ITERATIONS,
            encrypted: Vec::new(),
            iv: IV::default(),
            tag: Tag::default(),
            format: Box::new(self.to_owned()),
        };
        reader.read_exact(&mut wallet.iv)?;
        reader.read_exact(&mut wallet.salt)?;
        wallet.iterations = reader.read_u32::<LittleEndian>()?;
        reader.read_exact(&mut wallet.tag)?;
        reader.read_to_end(&mut wallet.encrypted)?;
        Ok(wallet)
    }

    fn write_wallet(&self, wallet: &Wallet, writer: &mut dyn io::Write) -> Result {
        writer.write_u16::<LittleEndian>(WALLET_KIND_BASIC)?;
        wallet.pubkey_bin.write(writer)?;
        writer.write_all(&wallet.iv)?;
        writer.write_all(&wallet.salt)?;
        writer.write_u32::<LittleEndian>(wallet.iterations)?;
        writer.write_all(&wallet.tag)?;
        writer.write_all(&wallet.encrypted)?;
        Ok(())
    }
}

impl WalletFormat for ShardedFormat {
    fn wallet_for_keypair(
        &mut self,
        keypair: &Keypair,
        password: &[u8],
        iterations: u32,
    ) -> Result<Wallet> {
        let mut salt = Salt::default();
        let mut stretched_key = AESKey::default();
        stretch_password(password, iterations, &mut salt, &mut stretched_key)?;

        let mut sss_key = SSSKey::default();
        randombytes::randombytes_into(&mut sss_key);

        let key_share_vecs =
            create_keyshares(&sss_key, self.key_share_count, self.recovery_threshold)?;
        let mut key_shares = Vec::new();
        for share_vec in key_share_vecs {
            key_shares.push(KeyShare::from_slice(&share_vec));
        }
        self.key_shares = key_shares;

        let encryption_key = derive_sharded_aes_key(&sss_key, &stretched_key)?;
        let mut wallet = Wallet {
            salt,
            iterations,
            pubkey_bin: PubKeyBin::default(),
            encrypted: Vec::new(),
            iv: IV::default(),
            tag: Tag::default(),
            format: Box::new(self.to_owned()),
        };

        encrypt_keypair(
            keypair,
            &encryption_key,
            &mut wallet.iv,
            &mut wallet.pubkey_bin,
            &mut wallet.encrypted,
            &mut wallet.tag,
        )?;
        Ok(wallet)
    }

    fn keypair_for_wallet(&self, wallet: &Wallet, password: &[u8]) -> Result<Keypair> {
        let mut stretched_key = AESKey::default();
        re_stretch_password(password, wallet.iterations, wallet.salt, &mut stretched_key)?;

        if self.key_shares.len() < self.recovery_threshold as usize {
            return Err("not enouth keyshares to recover key".into());
        }

        let mut sss_key = SSSKey::default();
        let key_share_vecs: Vec<Vec<u8>> = self.key_shares.iter().map(|sh| sh.to_vec()).collect();
        match combine_keyshares(&key_share_vecs) {
            Ok(k) => sss_key.copy_from_slice(&k),
            Err(_) => return Err("Failed to combine keyshares".into()),
        };
        let encryption_key = derive_sharded_aes_key(&sss_key, &stretched_key)?;

        let keypair = decrypt_keypair(
            &wallet.encrypted,
            &encryption_key,
            &wallet.pubkey_bin,
            &wallet.iv,
            &wallet.tag,
        )?;
        Ok(keypair)
    }

    fn as_sharded_format(&self) -> Result<ShardedFormat> {
        Ok(self.clone())
    }

    fn absorb_key_shares(&mut self, other: &ShardedFormat) -> Result {
        if self.key_share_count != other.key_share_count
            || self.recovery_threshold != other.recovery_threshold
        {
            return Err("Shards are not congruent".into());
        }
        self.key_shares.extend(other.key_shares.iter().cloned());
        Ok(())
    }

    fn read_wallet(&self, reader: &mut dyn io::Read) -> Result<Wallet> {
        let mut format = ShardedFormat::default();
        format.key_share_count = reader.read_u8()?;
        format.recovery_threshold = reader.read_u8()?;
        let mut key_share = KeyShare::default();
        reader.read_exact(&mut key_share.0)?;
        format.key_shares = vec![key_share];

        let mut wallet = Wallet {
            salt: Salt::default(),
            iterations: WALLET_DEFAULT_ITERATIONS,
            pubkey_bin: PubKeyBin::read(reader)?,
            encrypted: Vec::new(),
            iv: IV::default(),
            tag: Tag::default(),
            format: Box::new(format),
        };

        reader.read_exact(&mut wallet.iv)?;
        reader.read_exact(&mut wallet.salt)?;
        wallet.iterations = reader.read_u32::<LittleEndian>()?;
        reader.read_exact(&mut wallet.tag)?;
        reader.read_to_end(&mut wallet.encrypted)?;
        Ok(wallet)
    }

    fn write_wallet(&self, wallet: &Wallet, writer: &mut dyn io::Write) -> Result {
        writer.write_u16::<LittleEndian>(WALLET_KIND_SHARDED)?;
        writer.write_u8(self.key_share_count)?;
        writer.write_u8(self.recovery_threshold)?;
        writer.write_all(&self.key_shares[0].0)?;

        wallet.pubkey_bin.write(writer)?;
        writer.write_all(&wallet.iv)?;
        writer.write_all(&wallet.salt)?;
        writer.write_u32::<LittleEndian>(wallet.iterations)?;
        writer.write_all(&wallet.tag)?;
        writer.write_all(&wallet.encrypted)?;
        Ok(())
    }
}

impl Wallet {
    pub fn from_keypair(
        keypair: &Keypair,
        password: &[u8],
        iterations: u32,
        format: &mut dyn WalletFormat,
    ) -> Result<Wallet> {
        format.wallet_for_keypair(keypair, password, iterations)
    }

    pub fn to_keypair(&self, password: &[u8]) -> Result<Keypair> {
        self.format.keypair_for_wallet(self, password)
    }

    pub fn address(&self) -> Result<String> {
        self.pubkey_bin.to_b58()
    }

    pub fn is_sharded(&self) -> bool {
        self.format.as_sharded_format().is_ok()
    }
}

impl ReadWrite for Wallet {
    fn read(reader: &mut dyn io::Read) -> Result<Wallet> {
        let kind = reader.read_u16::<LittleEndian>()?;
        match kind {
            WALLET_KIND_BASIC => BasicFormat::default().read_wallet(reader),
            WALLET_KIND_SHARDED => ShardedFormat::default().read_wallet(reader),
            _ => Err(format!("Invalid wallet kind {}", kind).into()),
        }
    }

    fn write(&self, writer: &mut dyn io::Write) -> Result {
        self.format.write_wallet(self, writer)
    }
}

fn derive_sharded_aes_key(sss_key: &SSSKey, aes_key: &AESKey) -> Result<AESKey> {
    let mut hmac = match HmacSha256::new_varkey(sss_key) {
        Err(_) => return Err("Failed to initialize hmac".into()),
        Ok(m) => m,
    };
    hmac.input(aes_key);
    Ok(hmac.result().code().into())
}

//
// Utilities
//

pub fn stretch_password(
    password: &[u8],
    iterations: u32,
    salt: &mut Salt,
    key: &mut AESKey,
) -> Result {
    randombytes::randombytes_into(salt);
    re_stretch_password(password, iterations, *salt, key)
}

pub fn re_stretch_password(
    password: &[u8],
    iterations: u32,
    salt: Salt,
    key: &mut AESKey,
) -> Result {
    pbkdf2::pbkdf2::<Hmac<Sha256>>(password, &salt, iterations as usize, &mut key[..]);
    Ok(())
}

pub fn encrypt_keypair(
    keypair: &keypair::Keypair,
    key: &AESKey,
    iv: &mut IV,
    pubkey_bin: &mut PubKeyBin,
    encrypted: &mut Vec<u8>,
    tag: &mut Tag,
) -> Result {
    randombytes::randombytes_into(iv);

    let mut pubkey_writer: &mut [u8] = &mut pubkey_bin.0;
    keypair.public.write(&mut pubkey_writer)?;

    use aead::generic_array::GenericArray;
    let aead = Aes256Gcm::new(*GenericArray::from_slice(key));

    keypair.write(encrypted)?;
    match aead.encrypt_in_place_detached(iv.as_ref().into(), &pubkey_bin.0, encrypted) {
        Err(_) => Err("Failed to encrypt wallet".into()),
        Ok(gtag) => {
            tag.copy_from_slice(&gtag);
            Ok(())
        }
    }
}

pub fn decrypt_keypair(
    encrypted: &[u8],
    key: &AESKey,
    pubkey_bin: &PubKeyBin,
    iv: &IV,
    tag: &Tag,
) -> Result<Keypair> {
    use aead::generic_array::GenericArray;
    let aead = Aes256Gcm::new(*GenericArray::from_slice(key));
    let mut buffer = encrypted.to_owned();
    match aead.decrypt_in_place_detached(
        iv.as_ref().into(),
        &pubkey_bin.0,
        &mut buffer,
        tag.as_ref().into(),
    ) {
        Err(_) => Err("Failed to decrypt wallet"),
        _ => Ok(()),
    }?;
    let keypair = Keypair::read(&mut Cursor::new(buffer))?;
    Ok(keypair)
}

//
// Test
//

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rountrip_basic() {
        let from_keypair = Keypair::gen_keypair();
        let mut format = BasicFormat::default();
        let password = b"passsword";
        let wallet = Wallet::from_keypair(&from_keypair, password, 10, &mut format)
            .expect("wallet creation");
        let to_keypair = wallet.to_keypair(password).expect("wallet to keypair");
        assert_eq!(from_keypair, to_keypair);
    }

    #[test]
    fn rountrip_sharded() {
        let from_keypair = Keypair::gen_keypair();
        let mut format = ShardedFormat::default();
        let password = b"passsword";
        let wallet = Wallet::from_keypair(&from_keypair, password, 10, &mut format)
            .expect("wallet creation");
        let to_keypair = wallet.to_keypair(password).expect("wallet to keypair");
        assert_eq!(from_keypair, to_keypair);
    }
}
