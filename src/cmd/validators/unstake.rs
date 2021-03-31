use crate::{
    cmd::*,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign},
};

#[derive(Debug, StructOpt)]
/// Unstake a given validator. The stake will be in a cooldown period after
/// unstaking before the HNT is returned to the owning wallet.
pub struct Cmd {
    /// Address of the validator to unstake
    address: PublicKey,

    /// The amount of HNT of the original stake
    #[structopt(long)]
    stake_amount: Option<Hnt>,

    /// Manually set the fee to pay for the transaction
    #[structopt(long)]
    fee: Option<u64>,

    /// Whether to commit the transaction to the blockchain
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let client = helium_api::Client::new_with_base_url(api_url(wallet.public_key.network));

        let mut txn = BlockchainTxnUnstakeValidatorV1 {
            address: self.address.to_vec(),
            owner: wallet.public_key.to_vec(),
            stake_amount: if let Some(stake_amount) = self.stake_amount {
                u64::from(stake_amount)
            } else {
                helium_api::validators::get(&client, &self.address.to_string())
                    .await?
                    .stake
            },
            fee: 0,
            owner_signature: vec![],
        };

        txn.fee = if let Some(fee) = self.fee {
            fee
        } else {
            txn.txn_fee(&get_txn_fees(&client).await?)?
        };
        txn.owner_signature = txn.sign(&keypair)?;

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_txn(&envelope, &txn, &status, opts.format)
    }
}

fn print_txn(
    envelope: &BlockchainTxn,
    txn: &BlockchainTxnUnstakeValidatorV1,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let validator = PublicKey::from_bytes(&txn.address)?.to_string();
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Validator", validator],
                ["Fee", txn.fee],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "validator" : validator,
                "fee": txn.fee,
                "txn": envelope.to_b64()?,
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}
