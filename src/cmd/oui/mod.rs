use crate::{
    cmd::{print_footer, print_json, status_json, status_str, Opts, OutputFormat},
    keypair::PublicKey,
    result::{anyhow, Result},
    traits::b64::B64,
};
use helium_api::{BlockchainTxn, BlockchainTxnOuiV1, PendingTxnStatus};
use std::convert::TryInto;
use structopt::StructOpt;

mod create;
use create::*;
mod update;
use update::*;
mod submit;
use submit::*;

/// Create or update an OUI
#[derive(Debug, StructOpt)]
pub enum Cmd {
    Create(Create),
    Submit(Submit),
    Update(Update),
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Create(cmd) => cmd.run(opts),
            Cmd::Submit(cmd) => cmd.run(opts),
            Cmd::Update(cmd) => cmd.run(opts),
        }
    }
}

fn map_addresses<F, R>(addresses: Vec<impl TryInto<PublicKey>>, f: F) -> Result<Vec<R>>
where
    F: Fn(PublicKey) -> R,
{
    let results: Result<Vec<R>> = addresses
        .into_iter()
        .map(|v| match v.try_into() {
            Ok(public_key) => Ok(f(public_key)),
            Err(_err) => Err(anyhow!("failed to convert to public key")),
        })
        .collect();
    results
}

fn print_txn(
    txn: &BlockchainTxnOuiV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Requested OUI", txn.oui],
                ["Reqeuested Subnet Size", txn.requested_subnet_size],
                [
                    "Addresses",
                    map_addresses(txn.addresses.clone(), |v| v.to_string())?.join("\n")
                ],
                ["Hash", status_str(status)]
            );

            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "requested_oui": txn.oui + 1,
                "addresses": map_addresses(txn.addresses.clone(), |v| v.to_string())?,
                "requested_subnet_size": txn.requested_subnet_size,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });

            print_json(&table)
        }
    }
}
