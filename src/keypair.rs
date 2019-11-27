use crate::{
    result::Result,
    traits::{ReadWrite, B58},
};
use byteorder::ReadBytesExt;
use sodiumoxide::crypto::sign::ed25519;
use std::{fmt, io};

static START: std::sync::Once = std::sync::Once::new();
pub const KEYTYPE_ED25519: u8 = 1;

pub use ed25519::PublicKey;
pub use ed25519::SecretKey;
pub use ed25519::Seed;

// Newtype to allow us to `impl Default` on a 33 element array.
#[derive(Clone, Copy)]
pub struct PubKeyBin(pub(crate) [u8; 33]);

impl Default for PubKeyBin {
    fn default() -> Self {
        PubKeyBin([0; 33])
    }
}

impl From<&PublicKey> for PubKeyBin {
    fn from(pubkey: &PublicKey) -> Self {
        let mut buf= PubKeyBin::default();
        buf.0[0] = KEYTYPE_ED25519;
        buf.0[1..].copy_from_slice(&pubkey.0);
        buf
    }
}

impl PubKeyBin {
    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }

    pub fn from_vec(data: &[u8]) -> Self {
        let mut result= PubKeyBin::default();
        result.0.copy_from_slice(&data);
        result
    }
}

impl Into<PublicKey> for PubKeyBin {
    fn into(self) -> PublicKey {
        assert!(self.0[0] == KEYTYPE_ED25519);
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&self.0[1..]);
        PublicKey(buf)
    }
}

pub struct Keypair {
    pub public: PublicKey,
    pub secret: SecretKey,
}

fn init() {
    START.call_once(|| sodiumoxide::init().expect("Failed to intialize sodium"))
}

impl Keypair {
    pub fn gen_keypair() -> Keypair {
        init();
        let (pk, sk) = ed25519::gen_keypair();
        Keypair {
            public: pk,
            secret: sk,
        }
    }

    pub fn gen_keypair_from_seed(seed: &Seed) -> Keypair {
        init();
        let (pk, sk) = ed25519::keypair_from_seed(seed);
        Keypair {
            public: pk,
            secret: sk,
        }
    }

    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        ed25519::sign(data, &self.secret)
    }

    pub fn pubkey_bin(&self) -> PubKeyBin {
        PubKeyBin::from(&self.public)
    }
}

impl fmt::Display for Keypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Keypair({:?}, {:?})",
            self.public.to_b58().unwrap(),
            self.secret
        )
    }
}

impl fmt::Debug for Keypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl ReadWrite for Keypair {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&[KEYTYPE_ED25519])?;
        writer.write_all(&self.secret.0)?;
        writer.write_all(&self.public.0)?;
        Ok(())
    }

    fn read(reader: &mut dyn io::Read) -> Result<Keypair> {
        init();
        let key_type = reader.read_u8()?;
        if key_type != KEYTYPE_ED25519 {
            return Err(format!("Invalid key type {}", key_type).into());
        }

        let mut sk_buf = [0; 64];
        reader.read_exact(&mut sk_buf)?;

        let mut pk_buf = [0; 32];
        reader.read_exact(&mut pk_buf)?;

        Ok(Keypair {
            public: PublicKey(pk_buf),
            secret: SecretKey(sk_buf),
        })
    }
}

impl PartialEq for Keypair {
    fn eq(&self, other: &Self) -> bool {
        self.public == other.public && self.secret == other.secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_keypair() {
        let keypair = Keypair::gen_keypair();
        let mut buffer = Vec::new();
        keypair
            .write(&mut buffer)
            .expect("Failed to encode keypair");

        let decoded = Keypair::read(&mut Cursor::new(buffer)).expect("Failed to decode keypair");
        assert_eq!(keypair, decoded);
    }

    #[test]
    fn roundtrip_public_key() {
        let pk = Keypair::gen_keypair().public;
        let mut buffer = Vec::new();
        pk.write(&mut buffer).expect("Failed to encode public key");

        let decoded =
            PublicKey::read(&mut Cursor::new(buffer)).expect("Failed to decode public key");
        assert_eq!(pk, decoded);
    }

    #[test]
    fn roundtrip_b58_public_key() {
        let pk = Keypair::gen_keypair().public;
        let encoded = pk.to_b58().expect("Failed to encode public key");
        let decoded = PublicKey::from_b58(encoded).expect("Failed to decode public key");
        assert_eq!(pk, decoded);
    }
}
