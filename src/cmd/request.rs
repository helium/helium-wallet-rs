use crate::{cmd::*, result::Result};
use qr2term::print_qr;

#[derive(Debug, StructOpt)]
/// Construct various request (like payment) in a QR code
pub enum Cmd {
    Payment(Payment),
    Burn(Burn),
}

#[derive(Debug, StructOpt)]
/// Construct a payment request in a QR code with optional amount
pub struct Payment {
    #[structopt(long)]
    /// Amount of HNT to request
    amount: Option<Hnt>,
}

#[derive(Debug, StructOpt)]
/// Construct a token burn request in a QR code with optional amount and memo
/// fields.
pub struct Burn {
    /// Memo field to include. Provide as a base64 encoded string
    #[structopt(long)]
    memo: Option<String>,

    /// Amount of HNT to burn to DC
    #[structopt(long)]
    amount: Option<Hnt>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Payment(cmd) => cmd.run(opts).await,
            Cmd::Burn(cmd) => cmd.run(opts).await,
        }
    }
}

impl Payment {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;

        let mut request = json!({
            "type": "payment",
            "address": wallet.address()?,
        });
        if self.amount.is_some() {
            request["amount"] = self.amount.unwrap().to_string().into();
        }
        print_request(&request, opts.format)
    }
}

impl Burn {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;

        let mut request = json!({
            "type": "dc_burn",
            "address": wallet.address()?,
        });
        if self.amount.is_some() {
            request["amount"] = self.amount.unwrap().to_string().into();
        }
        if self.memo.is_some() {
            let value = self.memo.as_ref().unwrap();
            request["memo"] = value.clone().into();
        }
        print_request(&request, opts.format)
    }
}

fn print_request(request: &serde_json::Value, format: OutputFormat) -> Result {
    match format {
        OutputFormat::Json => print_json(request),
        OutputFormat::Table => {
            print_qr(&serde_json::to_string(&request)?)?;
            Ok(())
        }
    }
}
