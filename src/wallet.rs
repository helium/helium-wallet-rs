use crate::{
    format::{self, Format},
    keypair::{Keypair, PubKeyBin},
    pwhash::PWHash,
    result::Result,
    traits::{ReadWrite, B58},
};
use aead::NewAead;
use aes_gcm::Aes256Gcm;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use sodiumoxide::randombytes;
use std::io::{self, Cursor};

pub type Tag = [u8; 16];
pub type IV = [u8; 12];
pub type AESKey = [u8; 32];

const WALLET_KIND_BASIC_V1: u16 = 0x0001;
const WALLET_KIND_BASIC_V2: u16 = 0x0002;

const WALLET_KIND_SHARDED_V1: u16 = 0x0101;
const WALLET_KIND_SHARDED_V2: u16 = 0x0102;

const PWHASH_KIND_PBKDF2: u8 = 0;
const PWHASH_KIND_ARGON2ID13: u8 = 1;

pub struct Wallet {
    pub pubkey_bin: PubKeyBin,
    pub iv: IV,
    pub tag: Tag,
    pub encrypted: Vec<u8>,
    pub format: Format,
}

impl Wallet {
    pub fn encrypt(keypair: &Keypair, password: &[u8], fmt: Format) -> Result<Wallet> {
        let mut encryption_key = AESKey::default();
        let mut format = fmt;
        format.derive_key(password, &mut encryption_key)?;

        let mut iv = IV::default();
        randombytes::randombytes_into(&mut iv);

        let pubkey_bin = keypair.pubkey_bin();

        use aead::generic_array::GenericArray;
        let aead = Aes256Gcm::new(*GenericArray::from_slice(&encryption_key));

        let mut encrypted = vec![];
        keypair.write(&mut encrypted)?;

        match aead.encrypt_in_place_detached(iv.as_ref().into(), &pubkey_bin.0, &mut encrypted) {
            Err(_) => Err("Failed to encrypt wallet".into()),
            Ok(gtag) => Ok(Wallet {
                pubkey_bin,
                iv,
                tag: gtag.into(),
                encrypted,
                format,
            }),
        }
    }

    pub fn decrypt(&self, password: &[u8]) -> Result<Keypair> {
        let mut encryption_key = AESKey::default();
        let mut format = self.format.clone();
        format.derive_key(password, &mut encryption_key)?;

        use aead::generic_array::GenericArray;
        let aead = Aes256Gcm::new(*GenericArray::from_slice(&encryption_key));
        let mut buffer = self.encrypted.to_owned();
        match aead.decrypt_in_place_detached(
            self.iv.as_ref().into(),
            &self.pubkey_bin.0,
            &mut buffer,
            self.tag.as_ref().into(),
        ) {
            Err(_) => Err("Failed to decrypt wallet"),
            _ => Ok(()),
        }?;
        let keypair = Keypair::read(&mut Cursor::new(buffer))?;
        Ok(keypair)
    }

    pub fn address(&self) -> Result<String> {
        self.pubkey_bin.to_b58()
    }

    pub fn address_as_vec(&self) -> Vec<u8>{ self.pubkey_bin.to_vec()
    }

    pub fn pwhash(&self) -> &PWHash {
        self.format.pwhash()
    }

    fn mut_sharded_format(&mut self) -> Result<&mut format::Sharded> {
        match &mut self.format {
            Format::Sharded(format) => Ok(format),
            _ => Err("Wallet not sharded".into()),
        }
    }

    fn sharded_format(&self) -> Result<&format::Sharded> {
        match &self.format {
            Format::Sharded(format) => Ok(format),
            _ => Err("Wallet not sharded".into()),
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
                ..*self
            })
        }
        Ok(wallets)
    }

    pub fn absorb_shard(&mut self, shard: &Wallet) -> Result {
        let format = self.mut_sharded_format()?;
        let other_format = shard.sharded_format()?;

        format.absorb(&other_format)
    }

    fn read_pwhash(reader: &mut dyn io::Read) -> Result<PWHash> {
        let kind = reader.read_u8()?;
        match kind {
            PWHASH_KIND_PBKDF2 => Ok(PWHash::pbkdf2_default()),
            PWHASH_KIND_ARGON2ID13 => Ok(PWHash::argon2id13_default()),
            _ => Err(format!("Invalid pwhash kind {}", kind).into()),
        }
    }

    pub fn read(reader: &mut dyn io::Read) -> Result<Wallet> {
        let kind = reader.read_u16::<LittleEndian>()?;
        let mut format = match kind {
            WALLET_KIND_BASIC_V1 => Format::basic(PWHash::pbkdf2_default()),
            WALLET_KIND_BASIC_V2 => Format::basic(Self::read_pwhash(reader)?),
            WALLET_KIND_SHARDED_V1 => Format::sharded_default(PWHash::pbkdf2_default()),
            WALLET_KIND_SHARDED_V2 => Format::sharded_default(Self::read_pwhash(reader)?),
            _ => return Err(format!("Invalid wallet kind {}", kind).into()),
        };
        format.read(reader)?;
        let pubkey_bin = PubKeyBin::read(reader)?;
        let mut iv = IV::default();
        reader.read_exact(&mut iv)?;
        format.mut_pwhash().read(reader)?;
        let mut tag = Tag::default();
        reader.read_exact(&mut tag)?;
        let mut encrypted = vec![];
        reader.read_to_end(&mut encrypted)?;

        Ok(Wallet {
            pubkey_bin,
            iv,
            tag,
            format,
            encrypted,
        })
    }

    fn write_pwhash(pwhash: &PWHash, writer: &mut dyn io::Write) -> Result {
        match pwhash {
            PWHash::PBKDF2(_) => writer.write_u8(PWHASH_KIND_PBKDF2)?,
            PWHash::Argon2id13(_) => writer.write_u8(PWHASH_KIND_ARGON2ID13)?,
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
        self.pubkey_bin.write(writer)?;
        writer.write_all(&self.iv)?;
        self.format.pwhash().write(writer)?;
        writer.write_all(&self.tag)?;
        writer.write_all(&self.encrypted)?;
        Ok(())
    }
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
        let format = format::Basic {
            pwhash: PWHash::argon2id13_default(),
        };
        let password = b"passsword";
        let wallet = Wallet::encrypt(&from_keypair, password, Format::Basic(format))
            .expect("wallet creation");
        let to_keypair = wallet.decrypt(password).expect("wallet to keypair");
        assert_eq!(from_keypair, to_keypair);
    }

    #[test]
    fn rountrip_sharded() {
        let from_keypair = Keypair::gen_keypair();
        let format = format::Sharded {
            key_share_count: 5,
            recovery_threshold: 3,
            pwhash: PWHash::argon2id13_default(),
            key_shares: vec![],
        };
        let password = b"passsword";
        let wallet = Wallet::encrypt(&from_keypair, password, Format::Sharded(format))
            .expect("wallet creation");
        let to_keypair = wallet.decrypt(password).expect("wallet to keypair");
        assert_eq!(from_keypair, to_keypair);
    }
}
