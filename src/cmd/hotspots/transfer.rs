use crate::{
    cmd::*,
    keypair::PublicKey,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, B64},
};

#[derive(Debug, StructOpt)]
/// Transfer hotspot to a new owner
pub struct Cmd {
    /// Public key of the hotspot to be transferred
    gateway: PublicKey,
    /// The public key of the new owner of the hotspot
    new_owner: PublicKey,
    /// Commit the transaction to the blockchain
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;
        let client = new_client(api_url(wallet.public_key.network));

        let hotspot = helium_api::hotspots::get(&client, &self.gateway.to_string()).await?;
        // Get the next likely gateway nonce for the new transaction
        let nonce = hotspot.speculative_nonce + 1;

        let mut txn = BlockchainTxnTransferHotspotV2 {
            nonce,
            fee: 0,
            owner: wallet.public_key.to_vec(),
            gateway: self.gateway.into(),
            new_owner: self.new_owner.into(),
            owner_signature: vec![],
        };
        txn.fee = txn.txn_fee(&get_txn_fees(&client).await?)?;
        let password = get_password(false)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        txn.owner_signature = txn.sign(&keypair)?;

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnTransferHotspotV2,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let address = PublicKey::from_bytes(&txn.gateway)?.to_string();
    let new_owner = PublicKey::from_bytes(&txn.new_owner)?.to_string();
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Address", address],
                ["New Owner", new_owner],
                ["Nonce", txn.nonce],
                ["Fee (DC)", txn.fee],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "address": address,
                "new_owner": new_owner,
                "fee": txn.fee,
                "nonce": txn.nonce,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
        }
    }
}
