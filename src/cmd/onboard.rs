use crate::{
    cmd::{api_url, get_password, load_wallet, print_json, status_json, Opts, OutputFormat},
    result::Result,
    staking,
    traits::{Sign, TxnPayer, B64},
};
use helium_api::{BlockchainTxn, PendingTxnStatus, Txn};
use serde_json::json;
use std::io;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Use an onboarding key get a hotspot added or a location assertion
/// transaction signed by the Helium staking server.
pub struct Cmd {
    /// Base64 encoded transaction to sign. If no transaction if given
    /// stdin is read for the transaction. Note that the stdin feature
    /// only works if the wallet password is set in the
    /// HELIUM_WALLET_PASSWORD environment variable
    #[structopt(name = "TRANSACTION")]
    txn: Option<String>,

    /// The onboarding key to use if the payer of the transaction fees
    /// is the Helim "staking" server.
    #[structopt(long)]
    onboarding: Option<String>,

    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        // let staking_address = get_staking_address()?;
        // Now decode the given transaction
        let mut envelope = BlockchainTxn::from_b64(&self.read_txn()?)?;

        match &mut envelope.txn {
            Some(Txn::AddGateway(t)) => {
                t.owner_signature = t.sign(&keypair)?;
            }
            Some(Txn::AssertLocation(t)) => {
                t.owner_signature = t.sign(&keypair)?;
            }
            _ => return Err("Unsupported transaction for onboarding".into()),
        };

        // Check staking address
        let staking_client = staking::Client::default();
        let api_client = helium_api::Client::new_with_base_url(api_url());

        let wallet_key = keypair.pubkey_bin();

        let envelope = match envelope.payer()? {
            Some(key) if key == wallet_key => match &mut envelope.txn {
                Some(Txn::AddGateway(t)) => {
                    t.payer_signature = t.owner_signature.clone();
                    Ok(envelope)
                }
                Some(Txn::AssertLocation(t)) => {
                    t.payer_signature = t.owner_signature.clone();
                    Ok(envelope)
                }
                _ => Err("Unsupported transaction for onboarding"),
            },
            Some(_maker_key) => {
                if self.onboarding.is_none() {
                    Err("Staking server requires an onboarding key")
                } else {
                    let onboarding_key = self.onboarding.as_ref().unwrap().replace("\"", "");
                    Ok(staking_client.sign(&onboarding_key, &envelope)?)
                }
            }
            None => Ok(envelope),
        }?;

        let status = if self.commit {
            Some(api_client.submit_txn(&envelope)?)
        } else {
            None
        };
        print_txn(&envelope, &status, opts.format)
    }

    fn read_txn(&self) -> Result<String> {
        match &self.txn {
            Some(txn) => Ok(txn.to_string()),
            None => {
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
                Ok(buffer.trim().to_string())
            }
        }
    }
}

fn print_txn(
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let encoded = envelope.to_b64()?;
    match format {
        OutputFormat::Table => Err("Table format not supported for transaction output".into()),
        OutputFormat::Json => {
            let table = json!({
                "txn": encoded,
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}
