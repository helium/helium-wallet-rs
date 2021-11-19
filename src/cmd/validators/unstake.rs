use crate::{
    cmd::*,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign},
};

#[derive(Debug, StructOpt)]
/// Unstake a given validator. The stake will be in a cooldown period after
/// unstaking before the HNT is returned to the owning wallet.
///
/// The command requires a 'stake-release-height' argument which is suggested to
/// be at least the current block height plus the chain cooldown period (as
/// defined by a chain variable), and 5-10 blocks to allow for chain
/// processing delays.
pub struct Cmd {
    /// Address of the validator to unstake
    address: PublicKey,

    /// The amount of HNT of the original stake
    #[structopt(long)]
    stake_amount: Option<Hnt>,

    /// The stake release block height. This should be at least the current
    /// block height plus the cooldown period, and 5-10 blocks to allow for
    /// chain processing delays.
    #[structopt(long)]
    stake_release_height: u64,

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

        let client = new_client(api_url(wallet.public_key.network));

        let mut txn = BlockchainTxnUnstakeValidatorV1 {
            address: self.address.to_vec(),
            owner: wallet.public_key.to_vec(),
            stake_amount: if let Some(stake_amount) = self.stake_amount {
                u64::from(stake_amount)
            } else {
                u64::from(
                    helium_api::validators::get(&client, &self.address.to_string())
                        .await?
                        .stake,
                )
            },
            stake_release_height: self.stake_release_height,
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
                ["Fee (DC)", txn.fee],
                ["Stake (HNT)", Hnt::from(txn.stake_amount)],
                ["Hash", status_str(status)],
                [Frb => "WARNING",
                "After unstaking, a validator can not access the staked amount\n\
                nor earn rewards for 250,000 blocks (approx. five months)."]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "validator" : validator,
                "fee": txn.fee,
                "stake_amount": txn.stake_amount,
                "txn": envelope.to_b64()?,
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}
