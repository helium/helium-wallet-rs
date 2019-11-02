use crate::{
    keypair::{self, Keypair, PubKeyBin, PublicKey},
    result::Result,
    traits::ReadWrite,
};
use aead::NewAead;
use aes_gcm::Aes256Gcm;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use hmac::{Hmac, Mac};
use pbkdf2;
use sha2::Sha256;
use shamirsecretsharing::hazmat::{combine_keyshares, create_keyshares};
use sodiumoxide::randombytes;
use std::io::{self, Cursor};

pub type Salt = [u8; 8];
pub type Tag = [u8; 16];
pub type IV = [u8; 12];
pub type AESKey = [u8; 32];
pub type SSSKey = [u8; 32];

type HmacSha256 = Hmac<Sha256>;

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

pub enum Wallet {
    Basic(BasicWallet),
    Sharded(ShardedWallet),
}

impl ReadWrite for Wallet {
    fn read(reader: &mut dyn io::Read) -> Result<Wallet> {
        let kind = reader.read_u16::<LittleEndian>()?;
        match kind {
            0x0001 => {
                let wallet = BasicWallet::read(reader)?;
                Ok(Wallet::Basic(wallet))
            }
            0x0101 => {
                let wallet = ShardedWallet::read(reader)?;
                Ok(Wallet::Sharded(wallet))
            }
            _ => Err(format!("Invalid wallet type {}", kind).into()),
        }
    }

    fn write(&self, writer: &mut dyn io::Write) -> Result {
        match self {
            Wallet::Basic(wallet) => {
                writer.write_u16::<LittleEndian>(0x0001)?;
                wallet.write(writer)
            }
            Wallet::Sharded(wallet) => {
                writer.write_u16::<LittleEndian>(0x0101)?;
                wallet.write(writer)
            }
        }
    }
}

impl Wallet {
    pub fn is_sharded(&self) -> bool {
        match self {
            Wallet::Sharded(_) => true,
            _ => false,
        }
    }

    pub fn public_key(&self) -> &keypair::PublicKey {
        match self {
            Wallet::Basic(wallet) => wallet.public_key(),
            Wallet::Sharded(wallet) => wallet.public_key(),
        }
    }

    pub fn encrypt(&self, password: &[u8]) -> Result<Vec<Wallet>> {
        match self {
            Wallet::Basic(wallet) => match wallet {
                BasicWallet::Encrypted { .. } => Err("Wallet already encrypted".into()),
                BasicWallet::Decrypted { .. } => {
                    let mut salt = Salt::default();
                    let aes_key = self.derive_aes_key(password, Some(&mut salt))?;
                    let enc_wallet = wallet.encrypt(&aes_key, salt)?;
                    Ok(vec![Wallet::Basic(enc_wallet)])
                }
            },
            Wallet::Sharded(sh) => match sh {
                ShardedWallet::Encrypted { .. } => Err("Wallet already encrypted".into()),
                ShardedWallet::Decrypted {
                    recovery_threshold,
                    key_share_count,
                    ..
                } => {
                    let mut salt = Salt::default();
                    let aes_key = self.derive_aes_key(password, Some(&mut salt))?;

                    let mut sss_key = SSSKey::default();
                    randombytes::randombytes_into(&mut sss_key);
                    let final_key = derive_sharded_aes_key(&sss_key, &aes_key)?;

                    let key_shares =
                        create_keyshares(&sss_key, *key_share_count, *recovery_threshold)?;

                    let enc_wallet = sh.encrypt(&final_key, salt)?;

                    let mut wallets = Vec::with_capacity(key_shares.len());
                    for key_share in key_shares {
                        let wallet_share = Wallet::Sharded(enc_wallet.with_key_share(&key_share)?);
                        wallets.push(wallet_share);
                    }
                    Ok(wallets)
                }
            },
        }
    }

    pub fn decrypt(&self, password: &AESKey) -> Result<Wallet> {
        match self {
            Wallet::Basic(wallet) => {
                let dec_wallet = wallet.decrypt(password)?;
                Ok(Wallet::Basic(dec_wallet))
            }
            Wallet::Sharded(wallet) => {
                let dec_wallet = wallet.decrypt(password)?;
                Ok(Wallet::Sharded(dec_wallet))
            }
        }
    }

    fn derive_aes_key(&self, password: &[u8], out_salt: Option<&mut Salt>) -> Result<AESKey> {
        let mut aes_key = AESKey::default();
        match self {
            Wallet::Basic(BasicWallet::Encrypted {
                salt, iterations, ..
            }) => re_stretch_password(password, *iterations, *salt, &mut aes_key)?,
            Wallet::Basic(BasicWallet::Decrypted { iterations, .. }) => {
                stretch_password(password, *iterations, out_salt.unwrap(), &mut aes_key)?
            }
            Wallet::Sharded(ShardedWallet::Encrypted {
                salt, iterations, ..
            }) => re_stretch_password(password, *iterations, *salt, &mut aes_key)?,
            Wallet::Sharded(ShardedWallet::Decrypted { iterations, .. }) => {
                stretch_password(password, *iterations, out_salt.unwrap(), &mut aes_key)?
            }
        };
        Ok(aes_key)
    }

    pub fn decrypt_basic(password: &[u8], wallet: &Wallet) -> Result<Wallet> {
        let aes_key = wallet.derive_aes_key(password, None)?;
        wallet.decrypt(&aes_key)
    }

    pub fn decrypt_sharded(password: &[u8], shards: &[Wallet]) -> Result<Wallet> {
        let first_wallet = shards.first().expect("No wallet shards provided");
        let (k, n, aes_key) = match first_wallet {
            Wallet::Sharded(ShardedWallet::Encrypted {
                recovery_threshold,
                key_share_count,
                ..
            }) => {
                let aes_key = first_wallet.derive_aes_key(password, None)?;
                (*recovery_threshold, *key_share_count, aes_key)
            }
            _ => return Err("Not a sharded wallet".into()),
        };
        if shards.len() < k as usize {
            return Err("Not enough shards to recover the key".into());
        };
        // Check if shards are all encrypted and congruent
        let mut key_shares = Vec::new();
        for shard in shards {
            match shard {
                Wallet::Sharded(ShardedWallet::Encrypted {
                    recovery_threshold,
                    key_share_count,
                    key_share,
                    ..
                }) => {
                    if *recovery_threshold != k || *key_share_count != n {
                        return Err("Shards not congruent".into());
                    }
                    key_shares.push(key_share.to_vec());
                }
                _ => return Err("Not a sharded wallet".into()),
            }
        }

        let mut sss_key = SSSKey::default();
        match combine_keyshares(&key_shares) {
            Ok(k) => sss_key.copy_from_slice(&k),
            Err(_) => return Err("Failed to combine keyshares".into()),
        };
        let final_key = derive_sharded_aes_key(&sss_key, &aes_key)?;

        first_wallet.decrypt(&final_key)
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
