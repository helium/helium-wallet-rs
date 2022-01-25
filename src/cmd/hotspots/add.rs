use crate::{
    cmd::*,
    result::Result,
    staking,
    traits::{TxnEnvelope, TxnSign},
};

#[derive(Debug, StructOpt)]
/// Add a hotspot to the blockchain. The original transaction is created by the
/// hotspot miner and supplied here for owner signing. Use an onboarding key to
/// get the transaction signed by the DeWi staking server.
pub struct Cmd {
    /// Base64 encoded transaction to sign. If no transaction is given stdin is
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
    pub async fn run(self, opts: Opts) -> Result {
        let mut txn = BlockchainTxnAddGatewayV1::from_envelope(&read_txn(&self.txn)?)?;

        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let staking_client = staking::Client::default();
        let client = new_client(api_url(wallet.public_key.network));

        let wallet_key = keypair.public_key();

        txn.owner_signature = txn.sign(&keypair)?;
        let envelope = match PublicKey::from_bytes(&txn.payer)? {
            key if &key == wallet_key => {
                txn.payer_signature = txn.owner_signature.clone();
                Ok(txn.in_envelope())
            }
            _key if self.onboarding.is_some() && self.commit => {
                // Only have staking server sign if there's an onboarding key,
                // and we're actually going to commit
                let onboarding_key = self.onboarding.as_ref().unwrap().replace('\"', "");
                staking_client
                    .sign(&onboarding_key, &txn.in_envelope())
                    .await
            }
            _key => Ok(txn.in_envelope()),
        }?;

        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_txn(&txn, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnAddGatewayV1,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let address = PublicKey::from_bytes(&txn.gateway)?.to_string();
    let owner = PublicKey::from_bytes(&txn.owner)?.to_string();
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
                ["Payer", payer],
                ["Owner", owner],
                ["Fee (DC)", txn.fee],
                ["Staking fee (DC)", txn.staking_fee],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "address": address,
                "owner": owner,
                "payer": payer,
                "fee": txn.fee,
                "staking fee": txn.staking_fee,
                "hash": status_json(status),
                "txn": txn.in_envelope().to_b64()?
            });
            print_json(&table)
        }
    }
}
