use crate::{cmd::*, result::anyhow};
use std::convert::TryInto;
use structopt::StructOpt;

mod create;
use create::*;
mod update;
use update::*;

/// Create or update an OUI
#[derive(Debug, StructOpt)]
pub enum Cmd {
    Create(Box<Create>),
    Update(Update),
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Create(cmd) => cmd.run(opts).await,
            Cmd::Update(cmd) => cmd.run(opts).await,
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
