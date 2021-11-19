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
        let client = new_client(api_url(wallet.public_key.network));

        let status = maybe_submit_txn(true, &client, &envelope).await?;
        print_txn(&envelope, &status, opts.format)
    }
}

fn print_txn(
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            ptable!(["Key", "Value"], ["Hash", status_str(status)]);

            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "hash": status_json(status),
                "txn": envelope.to_b64()?
            });

            print_json(&table)
        }
    }
}
