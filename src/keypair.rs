use crate::{
    result::Result,
    traits::{Empty, ReadWrite, B58},
};
use byteorder::ReadBytesExt;
use sodiumoxide::crypto::sign::ed25519;
use std::{fmt, io};

static START: std::sync::Once = std::sync::Once::new();
pub const KEYTYPE_ED25519: u8 = 1;

pub use ed25519::PublicKey;
pub use ed25519::SecretKey;
pub type PubKeyBin = [u8; 33];

pub struct Keypair {
    pub public: PublicKey,
    pub secret: SecretKey,
}

fn init() {
    START.call_once(|| {
        if let Err(e) = sodiumoxide::init() {
            panic!("Failed to intialize sodium {:?}", e)
        }
    })
}

impl Empty for PubKeyBin {
    fn empty() -> Self {
        [0; 33]
    }
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
    fn write(&self, writer: &mut dyn io::Write) -> Result<()> {
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
