use crate::{
    pwhash::PwHash,
    result::{bail, Result},
};
use byteorder::{ReadBytesExt, WriteBytesExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use shamirsecretsharing::hazmat::{combine_keyshares, create_keyshares};
use sodiumoxide::randombytes;
use std::{fmt, io};

#[derive(Clone)]
pub enum Format {
    Basic(Basic),
    Sharded(Sharded),
}

impl Format {
    pub fn derive_key(&mut self, password: &[u8], key: &mut [u8]) -> Result {
        match self {
            Format::Basic(derive) => derive.derive_key(password, key),
            Format::Sharded(derive) => derive.derive_key(password, key),
        }
    }

    pub fn mut_pwhash(&mut self) -> &mut PwHash {
        match self {
            Format::Basic(derive) => derive.mut_pwhash(),
            Format::Sharded(derive) => derive.mut_pwhash(),
        }
    }

    pub fn pwhash(&self) -> &PwHash {
        match self {
            Format::Basic(derive) => derive.pwhash(),
            Format::Sharded(derive) => derive.pwhash(),
        }
    }

    pub fn read(&mut self, reader: &mut dyn io::Read) -> Result {
        match self {
            Format::Basic(derive) => derive.read(reader),
            Format::Sharded(derive) => derive.read(reader),
        }
    }

    pub fn write(&self, writer: &mut dyn io::Write) -> Result {
        match self {
            Format::Basic(derive) => derive.write(writer),
            Format::Sharded(derive) => derive.write(writer),
        }
    }

    pub fn basic(pwhash: PwHash) -> Self {
        Format::Basic(Basic { pwhash })
    }

    pub fn sharded(key_share_count: u8, recovery_threshold: u8, pwhash: PwHash) -> Self {
        Format::Sharded(Sharded {
            key_share_count,
            recovery_threshold,
            key_shares: Vec::new(),
            pwhash,
        })
    }

    pub fn sharded_default(pwhash: PwHash) -> Self {
        Self::sharded(5, 3, pwhash)
    }
}

#[derive(Clone)]
pub struct Basic {
    pub pwhash: PwHash,
}

impl Basic {
    pub fn derive_key(&mut self, password: &[u8], key: &mut [u8]) -> Result {
        self.pwhash.pwhash(password, key)
    }

    pub fn mut_pwhash(&mut self) -> &mut PwHash {
        &mut self.pwhash
    }

    pub fn pwhash(&self) -> &PwHash {
        &self.pwhash
    }

    pub fn read(&mut self, _reader: &mut dyn io::Read) -> Result {
        Ok(())
    }

    pub fn write(&self, _writer: &mut dyn io::Write) -> Result {
        Ok(())
    }
}

#[derive(Clone)]
pub struct KeyShare(pub(crate) [u8; 33]);

impl Default for KeyShare {
    fn default() -> Self {
        KeyShare([0; 33])
    }
}

impl fmt::Debug for KeyShare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("KeyShare").field(&&self.0[..]).finish()
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

#[derive(Clone, Debug)]
pub struct Sharded {
    pub key_share_count: u8,
    pub recovery_threshold: u8,
    pub key_shares: Vec<KeyShare>,
    pub pwhash: PwHash,
}

impl Sharded {
    pub fn derive_key(&mut self, password: &[u8], key: &mut [u8]) -> Result {
        self.pwhash.pwhash(password, key)?;

        let mut sss_key: [u8; 32] = [0; 32];

        if self.key_shares.is_empty() {
            // Generate the keyhares when we have none
            randombytes::randombytes_into(&mut sss_key);
            let key_share_vecs =
                create_keyshares(&sss_key, self.key_share_count, self.recovery_threshold)?;
            let mut key_shares = vec![];
            for share_vec in key_share_vecs {
                key_shares.push(KeyShare::from_slice(&share_vec));
            }
            self.key_shares = key_shares;
        } else if self.key_shares.len() < self.recovery_threshold as usize {
            // Otherwise validate that we can reconstruct the key
            bail!("not enouth keyshares to recover key");
        } else {
            // Reconstruct shared key
            let key_share_vecs: Vec<Vec<u8>> =
                self.key_shares.iter().map(|sh| sh.to_vec()).collect();
            match combine_keyshares(&key_share_vecs) {
                Ok(k) => sss_key.copy_from_slice(&k),
                Err(_) => bail!("Failed to combine keyshares"),
            }
        }

        // Now go derive the encryption key from the sharded key
        // source and the stretched key
        let mut hmac = match Hmac::<Sha256>::new_from_slice(&sss_key) {
            Err(_) => bail!("Failed to initialize hmac"),
            Ok(m) => m,
        };
        hmac.update(key);
        key.copy_from_slice(&hmac.finalize().into_bytes());
        Ok(())
    }

    pub fn mut_pwhash(&mut self) -> &mut PwHash {
        &mut self.pwhash
    }

    pub fn pwhash(&self) -> &PwHash {
        &self.pwhash
    }

    pub fn shards(&self) -> Vec<Self> {
        let mut shards = vec![];
        for share in &self.key_shares {
            shards.push(Self {
                key_shares: vec![share.clone()],
                ..*self
            })
        }
        shards
    }

    pub fn absorb(&mut self, other: &Self) -> Result {
        if self.key_share_count != other.key_share_count
            || self.recovery_threshold != other.recovery_threshold
        {
            bail!("Shards are not congruent");
        }

        self.key_shares.extend_from_slice(&other.key_shares);
        Ok(())
    }

    pub fn read(&mut self, reader: &mut dyn io::Read) -> Result {
        self.key_share_count = reader.read_u8()?;
        self.recovery_threshold = reader.read_u8()?;
        let mut key_share = KeyShare::default();
        reader.read_exact(&mut key_share.0)?;
        self.key_shares.push(key_share);
        Ok(())
    }

    pub fn write(&self, writer: &mut dyn io::Write) -> Result {
        if self.key_shares.len() != 1 {
            bail!("Invalid number of ksy shares in shard");
        }
        writer.write_u8(self.key_share_count)?;
        writer.write_u8(self.recovery_threshold)?;
        writer.write_all(&self.key_shares[0].0)?;
        Ok(())
    }
}
