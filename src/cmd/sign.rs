use crate::{cmd::*, result::Result};
use serde_json::json;
use std::{fs, path::PathBuf};

/// Commands for signing things
#[derive(Debug, StructOpt)]
pub enum Cmd {
    File(File),
    Msg(Msg),
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::File(cmd) => cmd.run(opts).await,
            Cmd::Msg(cmd) => cmd.run(opts).await,
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

fn print_signature(public_key: &PublicKey, signature: Vec<u8>) -> Result {
    let json = json!({
        "address": public_key.to_string(),
        "signature": base64::encode(&signature)
    });
    print_json(&json)
}
