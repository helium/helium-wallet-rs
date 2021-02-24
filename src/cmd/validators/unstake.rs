use crate::{
    cmd::*,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign},
};
use helium_api::{BlockchainTxnUnstakeValidatorV1, PendingTxnStatus};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Unstake a given validator. The stake will be in a cooldown period after
/// unstaking before the HNT is returned to the owning wallet.
pub struct Cmd {
    /// Address of the validator to unstake
    address: PublicKey,

    /// Whether to commit the transaction to the blockchain
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let client = helium_api::Client::new_with_base_url(api_url(wallet.public_key.network));

        let mut txn = BlockchainTxnUnstakeValidatorV1 {
            address: self.address.to_vec(),
            owner: wallet.public_key.to_vec(),
            fee: 0,
            owner_signature: vec![],
        };

        txn.fee = txn.txn_fee(&get_txn_fees(&client)?)?;
        txn.owner_signature = txn.sign(&keypair)?;

        let status = if self.commit {
            Some(client.submit_txn(&txn.in_envelope())?)
        } else {
            None
        };
        print_txn(&txn, &status, opts.format)
    }
}

fn print_txn(
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
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}
