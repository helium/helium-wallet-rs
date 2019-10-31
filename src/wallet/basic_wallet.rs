use crate::{
    keypair::{Keypair, PubKeyBin, PublicKey},
    result::Result,
    traits::{Empty, ReadWrite, B58},
    wallet::{self, AESKey, Salt, Tag, IV},
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::{fmt, io};

pub enum Wallet {
    Decrypted {
        keypair: Keypair,
        iterations: u32,
    },
    Encrypted {
        public_key: PublicKey,
        iv: IV,
        salt: Salt,
        iterations: u32,
        tag: Tag,
        encrypted: Vec<u8>,
    },
}

impl fmt::Display for Wallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Wallet::Encrypted { public_key, .. } => {
                write!(f, "Basic({})", public_key.to_b58().unwrap())
            }
            Wallet::Decrypted { keypair, .. } => {
                write!(f, "Basic({})", keypair.public.to_b58().unwrap())
            }
        }
    }
}

impl ReadWrite for Wallet {
    fn read(reader: &mut dyn io::Read) -> Result<Wallet> {
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
        let wallet = Wallet::Encrypted {
            public_key,
            iv,
            salt,
            iterations,
            tag,
            encrypted,
        };
        Ok(wallet)
    }

    fn write(&self, writer: &mut dyn io::Write) -> Result<()> {
        match self {
            Wallet::Decrypted { .. } => Err("not an encrypted wallet".into()),
            Wallet::Encrypted {
                public_key,
                iv,
                salt,
                iterations,
                tag,
                encrypted,
            } => {
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

impl Wallet {
    pub fn encrypt(&self, password: &AESKey, salt: Salt) -> Result<Self> {
        match self {
            Wallet::Encrypted { .. } => Err("not an decrypted wallet".into()),
            Wallet::Decrypted {
                iterations,
                keypair,
            } => {
                let mut pubkey_bin = PubKeyBin::empty();;
                let mut iv = IV::default();
                let mut tag = Tag::default();
                let mut encrypted = Vec::new();
                wallet::encrypt_keypair(
                    keypair,
                    password,
                    &mut iv,
                    &mut pubkey_bin,
                    &mut encrypted,
                    &mut tag,
                )?;
                let wallet = Wallet::Encrypted {
                    iterations: *iterations,
                    public_key: keypair.public,
                    salt,
                    iv,
                    tag,
                    encrypted,
                };

                Ok(wallet)
            }
        }
    }

    pub fn decrypt(&self, password: &AESKey) -> Result<Wallet> {
        match self {
            Wallet::Decrypted { .. } => Err("not an encrypted wallet".into()),
            Wallet::Encrypted {
                iterations,
                iv,
                encrypted,
                public_key,
                tag,
                ..
            } => {
                let keypair = wallet::decrypt_keypair(encrypted, &password, public_key, iv, tag)?;
                Ok(Wallet::Decrypted {
                    keypair,
                    iterations: *iterations,
                })
            }
        }
    }

    pub fn public_key(&self) -> &PublicKey {
        match self {
            Wallet::Encrypted { public_key, .. } => public_key,
            Wallet::Decrypted { keypair, .. } => &keypair.public,
        }
    }
}
