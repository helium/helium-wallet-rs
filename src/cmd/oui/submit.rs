use super::print_txn;
use crate::{
    cmd::{api_url, Opts},
    keypair::PublicKey,
    result::{anyhow, Result},
    traits::B64,
};
use helium_api::{BlockchainTxn, Txn};
use structopt::StructOpt;

/// Submits a given base64 oui transaction to the API. This command
/// can be used when this wallet is not the payer of the oui
/// transaction.
#[derive(Debug, StructOpt)]
pub struct Submit {
    /// Base64 encoded transaction to submit.
    #[structopt(name = "TRANSACTION")]
    transaction: String,

    /// Commit the payment to the API. If the staking server is used
    /// as the payer the transaction is first submitted to the staking
    /// server for signing and the result submitted ot the API.
    #[structopt(long)]
    commit: bool,
}

impl Submit {
    pub fn run(&self, opts: Opts) -> Result {
        let envelope = BlockchainTxn::from_b64(&self.transaction)?;
        if let Some(Txn::Oui(t)) = envelope.txn.clone() {
            let api_url = api_url(PublicKey::from_bytes(&t.owner)?.network);
            let api_client = helium_api::Client::new_with_base_url(api_url);
            let status = if self.commit {
                Some(api_client.submit_txn(&envelope)?)
            } else {
                None
            };
            print_txn(&t, &envelope, &status, opts.format)
        } else {
            Err(anyhow!("Invalid OUI transaction"))
        }
    }
}
