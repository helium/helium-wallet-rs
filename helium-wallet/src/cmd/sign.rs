use crate::cmd::*;
use serde_json::json;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: SubCmd,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

/// Commands for signing or verifying data
#[derive(Debug, clap::Subcommand)]
pub enum SubCmd {
    File(File),
    Msg(Msg),
    Verify(VerifyCmd),
}

impl SubCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::File(cmd) => cmd.run(opts).await,
            Self::Msg(cmd) => cmd.run(opts).await,
            Self::Verify(cmd) => cmd.run(opts).await,
        }
    }
}

/// Sign a given file
#[derive(Debug, clap::Args)]
pub struct File {
    /// Path to file to sign
    input: PathBuf,
}

impl File {
    pub async fn run(&self, opts: Opts) -> Result {
        use std::io::Read;
        let password = get_wallet_password(false)?;
        let wallet = opts.load_wallet()?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let mut data = Vec::new();
        fs::File::open(&self.input)?.read_to_end(&mut data)?;

        let signature = keypair.sign(&data)?;
        print_signature(&wallet, signature.as_ref())
    }
}

/// Sign a given message string
#[derive(Debug, clap::Args)]
pub struct Msg {
    /// Message to sign
    msg: String,
}

impl Msg {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = opts.load_wallet()?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let signature = keypair.sign(self.msg.as_bytes())?;
        print_signature(&wallet, signature.as_ref())
    }
}

/// Verify a file or message with a given signature
#[derive(clap::Args, Debug)]
pub struct VerifyCmd {
    #[command(subcommand)]
    cmd: Verify,
}

impl VerifyCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(clap::Subcommand, Debug)]
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
#[derive(clap::Args, Debug)]
pub struct VerifyFile {
    /// Path to file to sign
    input: PathBuf,

    /// Signature to verify
    #[arg(long, short)]
    signature: String,
}

impl VerifyFile {
    pub async fn run(&self, opts: Opts) -> Result {
        use helium_crypto::Verify;
        use std::io::Read;
        let wallet = opts.load_wallet()?;
        let mut data = Vec::new();
        fs::File::open(&self.input)?.read_to_end(&mut data)?;
        let signature = b64::decode(&self.signature)?;
        let verified = wallet.helium_pubkey()?.verify(&data, &signature).is_ok();
        print_verified(&wallet, verified)
    }
}

/// Verify the signature of a message
#[derive(clap::Args, Debug)]
pub struct VerifyMsg {
    /// Message to sign
    msg: String,

    /// Signature to verify
    #[arg(long, short)]
    signature: String,
}

impl VerifyMsg {
    pub async fn run(&self, opts: Opts) -> Result {
        use helium_crypto::Verify;
        let wallet = opts.load_wallet()?;
        let signature = b64::decode(&self.signature)?;
        let verified = wallet
            .helium_pubkey()?
            .verify(self.msg.as_bytes(), &signature)
            .is_ok();
        print_verified(&wallet, verified)
    }
}

fn json_address(wallet: &Wallet) -> Result<serde_json::Value> {
    let helium_address = wallet.helium_address()?;
    let address = wallet.address()?;
    Ok(json!({
        "solana": address,
        "helium": helium_address,
    }))
}

fn print_signature(wallet: &Wallet, signature: &[u8]) -> Result {
    let json = json!({
        "address": json_address(wallet)?,
        "signature": b64::encode(signature)
    });
    print_json(&json)
}

fn print_verified(wallet: &Wallet, verified: bool) -> Result {
    let json = json!({
        "address": json_address(wallet)?,
        "verified": verified
    });
    print_json(&json)
}
