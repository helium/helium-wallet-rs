use crate::{cmd::*, result::Result};
use serde_json::json;
use std::{fs, path::PathBuf};

/// Commands for signing or verifying data
#[derive(Debug, StructOpt)]
pub enum Cmd {
    File(File),
    Msg(Msg),
    Verify(Verify),
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::File(cmd) => cmd.run(opts).await,
            Self::Msg(cmd) => cmd.run(opts).await,
            Self::Verify(cmd) => cmd.run(opts).await,
        }
    }
}

/// Sign a given file
#[derive(Debug, StructOpt)]
pub struct File {
    /// Path to file to sign
    input: PathBuf,
}

impl File {
    pub async fn run(&self, opts: Opts) -> Result {
        use std::io::Read;
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let mut data = Vec::new();
        fs::File::open(&self.input)?.read_to_end(&mut data)?;

        let signature = keypair.sign(&data)?;
        print_signature(keypair.public_key(), signature)
    }
}

/// Sign a given message string
#[derive(Debug, StructOpt)]
pub struct Msg {
    /// Message to sign
    msg: String,
}

impl Msg {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let signature = keypair.sign(self.msg.as_bytes())?;
        print_signature(keypair.public_key(), signature)
    }
}

/// Verify a file or message with a given signature
#[derive(StructOpt, Debug)]
pub enum Verify {
    File(VerifyFile),
    Msg(VerifyMsg),
}

impl Verify {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::File(cmd) => cmd.run(opts).await,
            Self::Msg(cmd) => cmd.run(opts).await,
        }
    }
}

/// Verify the signature of a file
#[derive(StructOpt, Debug)]
pub struct VerifyFile {
    /// Path to file to sign
    input: PathBuf,

    /// Signature to verify
    #[structopt(long, short)]
    signature: String,
}

impl VerifyFile {
    pub async fn run(&self, opts: Opts) -> Result {
        use helium_crypto::Verify;
        use std::io::Read;
        let wallet = load_wallet(opts.files)?;
        let mut data = Vec::new();
        fs::File::open(&self.input)?.read_to_end(&mut data)?;
        let signature = base64::decode(&self.signature)?;
        let verified = wallet.public_key.verify(&data, &signature).is_ok();
        print_verified(&wallet.public_key, verified)
    }
}

/// Verify the signature of a message
#[derive(StructOpt, Debug)]
pub struct VerifyMsg {
    /// Message to sign
    msg: String,

    /// Signature to verify
    #[structopt(long, short)]
    signature: String,
}

impl VerifyMsg {
    pub async fn run(&self, opts: Opts) -> Result {
        use helium_crypto::Verify;
        let wallet = load_wallet(opts.files)?;
        let signature = base64::decode(&self.signature)?;
        let verified = wallet
            .public_key
            .verify(self.msg.as_bytes(), &signature)
            .is_ok();
        print_verified(&wallet.public_key, verified)
    }
}

fn print_signature(public_key: &PublicKey, signature: Vec<u8>) -> Result {
    let json = json!({
        "address": public_key.to_string(),
        "signature": base64::encode(&signature)
    });
    print_json(&json)
}

fn print_verified(public_key: &PublicKey, verified: bool) -> Result {
    let json = json!({
        "address": public_key.to_string(),
        "verified": verified
    });
    print_json(&json)
}
