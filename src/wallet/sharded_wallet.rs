use crate::{
    keypair::{Keypair, PublicKey},
    result::Result,
    traits::{ReadWrite, B58},
    wallet::{
        self, re_stretch_password, stretch_password, AESKey, SSSKey, Salt, Tag, Wallet, IV,
        WALLET_TYPE_BYTES_SHARDED,
    },
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use shamirsecretsharing::hazmat::create_keyshares;
use sodiumoxide::randombytes;
use std::{fmt, io};

type HmacSha256 = Hmac<Sha256>;

pub enum ShardedWallet {
    Decrypted {
        keypair: Keypair,
        iterations: u32,
        key_share_count: u8,
        recovery_threshold: u8,
    },
    Encrypted {
        public_key: PublicKey,
        iv: IV,
        salt: Salt,
        iterations: u32,
        tag: Tag,
        key_share_count: u8,
        recovery_threshold: u8,
        key_share: [u8; 33],
        encrypted: Vec<u8>,
    },
}

impl fmt::Display for ShardedWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShardedWallet::Encrypted { public_key, .. } => {
                write!(f, "Sharded({})", public_key.to_b58().unwrap())
            }
            ShardedWallet::Decrypted { keypair, .. } => {
                write!(f, "Sharded({})", keypair.public.to_b58().unwrap())
            }
        }
    }
}

impl ReadWrite for ShardedWallet {
    fn read(reader: &mut dyn io::Read) -> Result<ShardedWallet> {
        let key_share_count = reader.read_u8()?;
        let recovery_threshold = reader.read_u8()?;

        let mut key_share = [0; 33];
        reader.read_exact(&mut key_share)?;

        let public_key = PublicKey::read(reader)?;

        let mut iv = [0; 12];
        reader.read_exact(&mut iv)?;

        let mut salt = [0; 8];
        reader.read_exact(&mut salt)?;

        let iterations = reader.read_u32::<LittleEndian>()?;

        let mut tag = [0; 16];
        reader.read_exact(&mut tag)?;

        let mut encrypted = Vec::new();
        reader.read_to_end(&mut encrypted)?;
        let wallet = ShardedWallet::Encrypted {
            recovery_threshold,
            key_share_count,
            key_share,
            public_key,
            iv,
            salt,
            iterations,
            tag,
            encrypted,
        };
        Ok(wallet)
    }

    fn write(&self, writer: &mut dyn io::Write) -> Result {
        match self {
            ShardedWallet::Decrypted { .. } => Err("not an encrypted wallet".into()),
            ShardedWallet::Encrypted {
                recovery_threshold,
                key_share_count,
                key_share,
                public_key,
                iv,
                salt,
                iterations,
                tag,
                encrypted,
            } => {
                writer.write_u16::<LittleEndian>(WALLET_TYPE_BYTES_SHARDED)?;
                writer.write_u8(*key_share_count)?;
                writer.write_u8(*recovery_threshold)?;
                writer.write_all(key_share)?;
                public_key.write(writer)?;
                writer.write_all(iv)?;
                writer.write_all(salt)?;
                writer.write_u32::<LittleEndian>(*iterations)?;
                writer.write_all(tag)?;
                writer.write_all(encrypted)?;
                Ok(())
            }
        }
    }
}

impl Wallet for ShardedWallet {
    fn decrypt(&mut self, password: &[u8; 32]) -> Result<()> {
        match self {
            ShardedWallet::Decrypted { .. } => Err("not an encrypted wallet".into()),
            ShardedWallet::Encrypted {
                iterations,
                iv,
                encrypted,
                public_key,
                tag,
                key_share_count,
                recovery_threshold,
                ..
            } => {
                let keypair = wallet::decrypt_keypair(encrypted, password, public_key, iv, tag)?;
                *self = ShardedWallet::Decrypted {
                    keypair,
                    iterations: *iterations,
                    key_share_count: *key_share_count,
                    recovery_threshold: *recovery_threshold,
                };
                Ok(())
            }
        }
    }

    fn encrypt(&mut self, password: &AESKey, salt: Salt) -> Result<()> {
        match self {
            ShardedWallet::Encrypted { .. } => Err("not an decrypted wallet".into()),
            ShardedWallet::Decrypted {
                iterations,
                keypair,
                key_share_count,
                recovery_threshold,
            } => {
                let (iv, tag, encrypted) = wallet::encrypt_keypair(keypair, password)?;

                *self = ShardedWallet::Encrypted {
                    key_share_count: *key_share_count,
                    recovery_threshold: *recovery_threshold,
                    iterations: *iterations,
                    public_key: keypair.public,
                    salt,
                    iv,
                    tag,
                    encrypted,
                    key_share: [0; 33],
                };
                Ok(())
            }
        }
    }

    fn public_key(&self) -> &PublicKey {
        match self {
            ShardedWallet::Encrypted { public_key, .. } => public_key,
            ShardedWallet::Decrypted { keypair, .. } => &keypair.public,
        }
    }

    fn derive_aes_key(&self, password: &[u8], out_salt: Option<&mut Salt>) -> Result<AESKey> {
        let mut aes_key = AESKey::default();
        match self {
            ShardedWallet::Encrypted {
                salt, iterations, ..
            } => re_stretch_password(password, *iterations, *salt, &mut aes_key)?,
            ShardedWallet::Decrypted { iterations, .. } => {
                stretch_password(password, *iterations, out_salt.unwrap(), &mut aes_key)?
            }
        };
        Ok(aes_key)
    }
}

impl ShardedWallet {
    pub fn num_shards(&self) -> u8 {
        match self {
            ShardedWallet::Decrypted {
                key_share_count, ..
            } => *key_share_count,
            ShardedWallet::Encrypted {
                key_share_count, ..
            } => *key_share_count,
        }
    }

    fn with_key_share(&self, share: &[u8]) -> Result<ShardedWallet> {
        match self {
            ShardedWallet::Decrypted { .. } => Err("not an encrypted wallet".into()),
            ShardedWallet::Encrypted {
                public_key,
                iv,
                salt,
                iterations,
                tag,
                key_share_count,
                recovery_threshold,
                encrypted,
                ..
            } => {
                let mut key_share = [0u8; 33];
                key_share.copy_from_slice(share);
                let wallet = ShardedWallet::Encrypted {
                    public_key: *public_key,
                    iv: *iv,
                    salt: *salt,
                    iterations: *iterations,
                    tag: *tag,
                    key_share_count: *key_share_count,
                    recovery_threshold: *recovery_threshold,
                    key_share,
                    encrypted: encrypted.to_vec(),
                };
                Ok(wallet)
            }
        }
    }

    pub fn create(
        iterations: u32,
        key_share_count: u8,
        recovery_threshold: u8,
        password: &[u8],
    ) -> Result<Vec<ShardedWallet>> {
        let keypair = Keypair::gen_keypair();
        let mut wallet = ShardedWallet::Decrypted {
            iterations,
            keypair,
            key_share_count,
            recovery_threshold,
        };
        let mut salt = Salt::default();
        let mut aes_key = AESKey::default();
        stretch_password(password, iterations, &mut salt, &mut aes_key)?;
        let mut sss_key = SSSKey::default();
        randombytes::randombytes_into(&mut sss_key);
        let final_key = derive_sharded_aes_key(&sss_key, &aes_key)?;
        let key_shares = create_keyshares(&sss_key, key_share_count, recovery_threshold)?;
        wallet.encrypt(&final_key, salt)?;
        let mut wallets = Vec::with_capacity(key_shares.len());
        for key_share in key_shares {
            let wallet_share = wallet.with_key_share(&key_share)?;
            wallets.push(wallet_share);
        }
        Ok(wallets)
    }
}

pub fn derive_sharded_aes_key(sss_key: &SSSKey, aes_key: &AESKey) -> Result<AESKey> {
    let mut hmac = match HmacSha256::new_varkey(sss_key) {
        Err(_) => return Err("Failed to initialize hmac".into()),
        Ok(m) => m,
    };
    hmac.input(aes_key);
    Ok(hmac.result().code().into())
}