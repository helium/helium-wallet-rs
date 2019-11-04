use crate::{
    keypair::{Keypair, PublicKey},
    result::Result,
    traits::{ReadWrite, B58},
    wallet::{
        self, re_stretch_password, stretch_password, AESKey, Salt, Tag, Wallet, IV,
        WALLET_TYPE_BYTES_BASIC,
    },
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::{fmt, io};

pub enum BasicWallet {
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

impl fmt::Display for BasicWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BasicWallet::Encrypted { public_key, .. } => {
                write!(f, "Basic({})", public_key.to_b58().unwrap())
            }
            BasicWallet::Decrypted { keypair, .. } => {
                write!(f, "Basic({})", keypair.public.to_b58().unwrap())
            }
        }
    }
}

impl ReadWrite for BasicWallet {
    fn read(reader: &mut dyn io::Read) -> Result<BasicWallet> {
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
        let wallet = BasicWallet::Encrypted {
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
            BasicWallet::Decrypted { .. } => Err("not an encrypted wallet".into()),
            BasicWallet::Encrypted {
                public_key,
                iv,
                salt,
                iterations,
                tag,
                encrypted,
            } => {
                writer.write_u16::<LittleEndian>(WALLET_TYPE_BYTES_BASIC)?;
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

impl BasicWallet {
    pub fn create(iterations: u32, password: &[u8]) -> Result<BasicWallet> {
        let keypair = Keypair::gen_keypair();
        let mut wallet = BasicWallet::Decrypted {
            keypair,
            iterations,
        };
        let mut salt = Salt::default();
        let mut aes_key = AESKey::default();
        stretch_password(password, iterations, &mut salt, &mut aes_key)?;
        wallet.encrypt(&aes_key, salt)?;
        Ok(wallet)
    }
}

impl Wallet for BasicWallet {
    fn decrypt(&mut self, password: &[u8; 32]) -> Result<()> {
        match self {
            BasicWallet::Decrypted { .. } => Err("not an encrypted wallet".into()),
            BasicWallet::Encrypted {
                iterations,
                iv,
                encrypted,
                public_key,
                tag,
                ..
            } => {
                let keypair = wallet::decrypt_keypair(encrypted, password, public_key, iv, tag)?;
                *self = BasicWallet::Decrypted {
                    keypair,
                    iterations: *iterations,
                };
                Ok(())
            }
        }
    }

    fn encrypt(&mut self, password: &AESKey, salt: Salt) -> Result<()> {
        match self {
            BasicWallet::Encrypted { .. } => Err("Wallet already encrypted".into()),
            BasicWallet::Decrypted {
                iterations,
                keypair,
            } => {
                let (iv, tag, encrypted) = wallet::encrypt_keypair(keypair, password)?;
                *self = BasicWallet::Encrypted {
                    iterations: *iterations,
                    public_key: keypair.public,
                    salt,
                    iv,
                    tag,
                    encrypted,
                };
                Ok(())
            }
        }
    }

    fn public_key(&self) -> &PublicKey {
        match self {
            BasicWallet::Encrypted { public_key, .. } => public_key,
            BasicWallet::Decrypted { keypair, .. } => &keypair.public,
        }
    }

    fn derive_aes_key(&self, password: &[u8], out_salt: Option<&mut Salt>) -> Result<AESKey> {
        let mut aes_key = AESKey::default();
        match self {
            BasicWallet::Encrypted {
                salt, iterations, ..
            } => re_stretch_password(password, *iterations, *salt, &mut aes_key)?,
            BasicWallet::Decrypted { iterations, .. } => {
                stretch_password(password, *iterations, out_salt.unwrap(), &mut aes_key)?
            }
        };
        Ok(aes_key)
    }
}