use crate::result::Result;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use hmac::Hmac;
use sha2::Sha256;
use sodiumoxide::{crypto::pwhash::argon2id13, randombytes};
use std::{convert::TryInto, fmt, io};

#[derive(Clone, Copy, Debug)]
pub enum PWHash {
    PBKDF2(PBKDF2),
    Argon2id13(Argon2id13),
}

impl PWHash {
    pub fn pwhash(&self, password: &[u8], hash: &mut [u8]) -> Result {
        match self {
            PWHash::PBKDF2(hasher) => hasher.pwhash(password, hash),
            PWHash::Argon2id13(hasher) => hasher.pwhash(password, hash),
        }
    }

    pub fn read(&mut self, reader: &mut dyn io::Read) -> Result {
        match self {
            PWHash::PBKDF2(hasher) => hasher.read(reader),
            PWHash::Argon2id13(hasher) => hasher.read(reader),
        }
    }

    pub fn write(&self, writer: &mut dyn io::Write) -> Result {
        match self {
            PWHash::PBKDF2(hasher) => hasher.write(writer),
            PWHash::Argon2id13(hasher) => hasher.write(writer),
        }
    }

    pub fn pbkdf2_default() -> Self {
        PWHash::PBKDF2(PBKDF2::with_iterations(PBKDF2_DEFAULT_ITERATIONS))
    }

    pub fn pbkdf2(iterations: u32) -> Self {
        PWHash::PBKDF2(PBKDF2::with_iterations(iterations))
    }

    pub fn argon2id13_default() -> Self {
        PWHash::Argon2id13(Argon2id13::default())
    }
}

impl fmt::Display for PWHash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PWHash::PBKDF2(_) => f.write_str("PBKDF2"),
            PWHash::Argon2id13(_) => f.write_str("Argon2id13"),
        }
    }
}

pub const PBKDF2_DEFAULT_ITERATIONS: u32 = 1_000_000;

#[derive(Clone, Copy, Debug)]
pub struct PBKDF2 {
    salt: [u8; 8],
    iterations: u32,
}

impl PBKDF2 {
    pub fn with_iterations(iterations: u32) -> Self {
        let mut salt: [u8; 8] = [0; 8];
        randombytes::randombytes_into(&mut salt);
        Self { salt, iterations }
    }

    pub fn pwhash(&self, password: &[u8], hash: &mut [u8]) -> Result {
        pbkdf2::pbkdf2::<Hmac<Sha256>>(password, &self.salt, self.iterations as usize, hash);
        Ok(())
    }

    pub fn read(&mut self, reader: &mut dyn io::Read) -> Result {
        reader.read_exact(&mut self.salt)?;
        self.iterations = reader.read_u32::<LittleEndian>()?;
        Ok(())
    }

    pub fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&self.salt)?;
        writer.write_u32::<LittleEndian>(self.iterations)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Argon2id13 {
    salt: argon2id13::Salt,
    mem_limit: argon2id13::MemLimit,
    ops_limit: argon2id13::OpsLimit,
}

impl Default for Argon2id13 {
    fn default() -> Self {
        Self::with_limits(
            argon2id13::OPSLIMIT_SENSITIVE,
            argon2id13::MEMLIMIT_SENSITIVE,
        )
    }
}

impl Argon2id13 {
    pub fn with_limits(ops_limit: argon2id13::OpsLimit, mem_limit: argon2id13::MemLimit) -> Self {
        Self {
            salt: argon2id13::gen_salt(),
            mem_limit,
            ops_limit,
        }
    }

    pub fn pwhash(&self, password: &[u8], hash: &mut [u8]) -> Result {
        match argon2id13::derive_key(hash, password, &self.salt, self.ops_limit, self.mem_limit) {
            Ok(_) => Ok(()),
            Err(_) => Err("Failed to hash password".into()),
        }
    }

    pub fn read(&mut self, reader: &mut dyn io::Read) -> Result {
        reader.read_exact(&mut self.salt.0)?;
        self.mem_limit = argon2id13::MemLimit(reader.read_u32::<LittleEndian>()?.try_into()?);
        self.ops_limit = argon2id13::OpsLimit(reader.read_u32::<LittleEndian>()?.try_into()?);
        Ok(())
    }

    pub fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&self.salt.0)?;
        writer.write_u32::<LittleEndian>(self.mem_limit.0.try_into()?)?;
        writer.write_u32::<LittleEndian>(self.ops_limit.0.try_into()?)?;
        Ok(())
    }
}
