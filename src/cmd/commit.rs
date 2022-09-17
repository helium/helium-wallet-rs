use crate::{cmd::*, result::Result};
use serde_json::json;

#[derive(Debug, StructOpt)]
/// Commit a transaction to the blockchain.
///
/// The transaction is submitted to the network that the current wallet is
/// active on (mainnet or testnet)
pub struct Cmd {
    /// Base64 encoded transaction to sign. If no transaction is given stdin is
    /// read for the transaction.
    #[structopt(name = "TRANSACTION")]
    txn: Option<Transaction>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let envelope = read_txn(&self.txn)?;
        let wallet = load_wallet(opts.files)?;
        let base_url: String = api_url(wallet.public_key.network);
        let client = new_client(base_url.clone());
        let status = maybe_submit_txn(true, &client, &envelope).await?;
        let pending_url = base_url + "/pending_transactions/";

        print_txn(&envelope, &status, &pending_url, opts.format)
    }
}

fn print_txn(
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    pending_url: &str,
    format: OutputFormat,
) -> Result {
    let status_endpoint = pending_url.to_owned() + status_str(status);
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Hash", status_str(status)],
                ["Status", status_endpoint]
            );

            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
                "status": status_endpoint
            });

            print_json(&table)
        }
    }
}
