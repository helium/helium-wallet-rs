use crate::{
    keypair::{self, Keypair, PublicKey},
    result::Result,
    traits::ReadWrite,
};
use aead::NewAead;
use aes_gcm::Aes256Gcm;
use byteorder::{LittleEndian, ReadBytesExt};
use hmac::{Hmac, Mac};
use pbkdf2;
use sha2::Sha256;
use shamirsecretsharing::hazmat::combine_keyshares;
use sodiumoxide::randombytes;
use std::io::{self, Cursor};

pub type Salt = [u8; 8];
pub type Tag = [u8; 16];
pub type IV = [u8; 12];
pub type AESKey = [u8; 32];
pub type SSSKey = [u8; 32];

const WALLET_TYPE_BYTES_BASIC: u16 = 0x0001;
const WALLET_TYPE_BYTES_SHARDED: u16 = 0x0101;

mod basic_wallet;
use basic_wallet::BasicWallet;
pub mod basic {
    pub use crate::wallet::basic_wallet::*;
}
mod sharded_wallet;
use sharded_wallet::ShardedWallet;
pub mod sharded {
    pub use crate::wallet::sharded_wallet::*;
}

pub enum WalletReadWrite {
    Basic(BasicWallet),
    Sharded(ShardedWallet),
}

impl ReadWrite for WalletReadWrite {
    fn read(reader: &mut dyn io::Read) -> Result<WalletReadWrite> {
        let kind = reader.read_u16::<LittleEndian>()?;
        match kind {
            WALLET_TYPE_BYTES_BASIC => {
                let wallet = BasicWallet::read(reader)?;
                Ok(WalletReadWrite::Basic(wallet))
            }
            WALLET_TYPE_BYTES_SHARDED => {
                let wallet = ShardedWallet::read(reader)?;
                Ok(WalletReadWrite::Sharded(wallet))
            }
            _ => Err(format!("Invalid wallet type {}", kind).into()),
        }
    }

    fn write(&self, writer: &mut dyn io::Write) -> Result {
        match self {
            WalletReadWrite::Basic(wallet) => wallet.write(writer),
            WalletReadWrite::Sharded(wallet) => wallet.write(writer),
        }
    }
}

pub trait Wallet {
    fn public_key(&self) -> &keypair::PublicKey;
    fn decrypt(&mut self, password: &[u8; 32]) -> Result<()>;
    fn encrypt(&mut self, password: &AESKey, salt: Salt) -> Result<()>;
    fn derive_aes_key(&self, password: &[u8], out_salt: Option<&mut Salt>) -> Result<AESKey>;
}

pub fn decrypt_basic(password: &[u8], mut wallet: BasicWallet) -> Result<BasicWallet> {
    let aes_key = wallet.derive_aes_key(password, None)?;
    wallet.decrypt(&aes_key)?;
    Ok(wallet)
}

pub fn decrypt_sharded(password: &[u8], mut shards: Vec<ShardedWallet>) -> Result<ShardedWallet> {
    let first_wallet = shards.first().expect("No wallet shards provided");
    let (k, n, aes_key) = match first_wallet {
        ShardedWallet::Encrypted {
            recovery_threshold,
            key_share_count,
            iterations,
            salt,
            ..
        } => {
            let mut aes_key = AESKey::default();
            re_stretch_password(password, *iterations, *salt, &mut aes_key)?;
            (*recovery_threshold, *key_share_count, aes_key)
        }
        _ => return Err("Not an encrypted wallet".into()),
    };
    if shards.len() < k as usize {
        return Err("Not enough shards to recover the key".into());
    };
    // Check if shards are all encrypted and congruent
    let mut key_shares = Vec::new();
    for shard in shards.iter() {
        match shard {
            ShardedWallet::Encrypted {
                recovery_threshold,
                key_share_count,
                key_share,
                ..
            } => {
                if *recovery_threshold != k || *key_share_count != n {
                    return Err("Shards not congruent".into());
                }
                key_shares.push(key_share.to_vec());
            }
            _ => return Err("Not an encrypted wallet".into()),
        }
    }

    let mut sss_key = SSSKey::default();
    match combine_keyshares(&key_shares) {
        Ok(k) => sss_key.copy_from_slice(&k),
        Err(_) => return Err("Failed to combine keyshares".into()),
    };
    let final_key = sharded_wallet::derive_sharded_aes_key(&sss_key, &aes_key)?;

    shards[0].decrypt(&final_key)?;
    Ok(shards.remove(0))
}

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

pub fn encrypt_keypair(keypair: &keypair::Keypair, key: &AESKey) -> Result<(IV, Tag, Vec<u8>)> {
    let mut pubkey_bin = [0; 33];
    let mut iv = IV::default();
    let mut tag = Tag::default();
    let mut encrypted = Vec::new();
    randombytes::randombytes_into(&mut iv);
    let mut pubkey_writer: &mut [u8] = &mut pubkey_bin;
    keypair.public.write(&mut pubkey_writer)?;

    use aead::generic_array::GenericArray;
    let aead = Aes256Gcm::new(*GenericArray::from_slice(key));

    keypair.write(&mut encrypted)?;
    match aead.encrypt_in_place_detached(iv.as_ref().into(), &mut pubkey_bin, &mut encrypted) {
        Err(_) => Err("Failed to encrypt wallet".into()),
        Ok(gtag) => {
            tag.copy_from_slice(&gtag);
            Ok((iv, tag, encrypted))
        }
    }
}

pub fn decrypt_keypair(
    encrypted: &[u8],
    key: &AESKey,
    public_key: &PublicKey,
    iv: &IV,
    tag: &Tag,
) -> Result<Keypair> {
    use aead::generic_array::GenericArray;
    let aead = Aes256Gcm::new(*GenericArray::from_slice(key));
    let mut buffer = encrypted.to_owned();
    let mut pubkey_bin = Vec::new();
    public_key.write(&mut pubkey_bin)?;
    match aead.decrypt_in_place_detached(
        iv.as_ref().into(),
        &pubkey_bin,
        &mut buffer,
        tag.as_ref().into(),
    ) {
        Err(_) => Err("Failed to decrypt wallet"),
        _ => Ok(()),
    }?;
    let keypair = Keypair::read(&mut Cursor::new(buffer))?;
    Ok(keypair)
}