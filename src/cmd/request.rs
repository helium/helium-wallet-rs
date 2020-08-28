use crate::{
    cmd::{load_wallet, print_json, Opts, OutputFormat},
    result::Result,
};
use helium_api::Hnt;
use qr2term::print_qr;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Construct various request (like payment) in a QR code
pub enum Cmd {
    Payment(Payment),
}

#[derive(Debug, StructOpt)]
/// Construct a payment request in a QR code
pub struct Payment {
    #[structopt(long)]
    /// Amount of HNT to request
    amount: Option<Hnt>,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Payment(cmd) => cmd.run(opts),
        }
    }
}

impl Payment {
    pub fn run(&self, opts: Opts) -> Result {
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

fn print_request(request: &serde_json::Value, format: OutputFormat) -> Result {
    match format {
        OutputFormat::Json => print_json(request),
        OutputFormat::Table => {
            print_qr(&serde_json::to_string(&request)?)?;
            Ok(())
        }
    }
}
