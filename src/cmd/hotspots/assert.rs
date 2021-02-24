use crate::{
    cmd::*,
    result::{bail, Result},
    staking,
    traits::{TxnEnvelope, TxnSign},
};
use helium_api::{BlockchainTxnAssertLocationV1, PendingTxnStatus};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Assert a hotspot location on the blockchain. The original transaction is
/// created by the hotspot miner and supplied here for owner signing. Use an
/// onboarding key to get the transaction signed by the DeWi staking server.
pub struct Cmd {
    /// Base64 encoded transaction to sign. If no transaction if given stdin is
    /// read for the transaction. Note that the stdin feature only works if the
    /// wallet password is set in the HELIUM_WALLET_PASSWORD environment
    /// variable
    #[structopt(name = "TRANSACTION")]
    txn: Option<Transaction>,

    /// The onboarding key to use if the payer of the transaction fees
    /// is the DeWi "staking" server.
    #[structopt(long)]
    onboarding: Option<String>,

    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(self, opts: Opts) -> Result {
        let mut txn = BlockchainTxnAssertLocationV1::from_envelope(&read_txn(&self.txn)?)?;

        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let staking_client = staking::Client::default();
        let client = helium_api::Client::new_with_base_url(api_url(wallet.public_key.network));

        let wallet_key = keypair.public_key();

        txn.owner_signature = txn.sign(&keypair)?;
        let envelope = match PublicKey::from_bytes(&txn.payer)? {
            key if &key == wallet_key => {
                txn.payer_signature = txn.owner_signature.clone();
                Ok(txn.in_envelope())
            }
            _maker_key => {
                if self.onboarding.is_none() {
                    bail!("Staking server requires an onboarding key");
                } else {
                    let onboarding_key = self.onboarding.as_ref().unwrap().replace("\"", "");
                    staking_client.sign(&onboarding_key, &txn.in_envelope())
                }
            }
        }?;

        let status = if self.commit {
            Some(client.submit_txn(&envelope)?)
        } else {
            None
        };
        print_txn(&txn, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnAssertLocationV1,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let address = PublicKey::from_bytes(&txn.gateway)?.to_string();
    let payer = if txn.payer.is_empty() {
        PublicKey::from_bytes(&txn.owner)?.to_string()
    } else {
        PublicKey::from_bytes(&txn.payer)?.to_string()
    };
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Address", address],
                ["Location", txn.location],
                ["Payer", payer],
                ["Fee", txn.fee],
                ["Staking fee", txn.staking_fee],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "address": address,
                "location": txn.location,
                "payer": payer,
                "fee": txn.fee,
                "staking fee": txn.staking_fee,
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}
