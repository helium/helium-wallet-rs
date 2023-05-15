use crate::{b64, cmd::*, keypair::Pubkey, result::Result};
use rust_decimal::prelude::*;
use serde_json::json;

#[derive(Debug, clap::Args)]
/// Burn HNT to Data Credits (DC) from this wallet to given payees wallet.
pub struct Cmd {
    /// Account address to send the resulting DC to. Defaults to the active
    /// wallet.
    #[arg(long)]
    payee: Option<Pubkey>,

    /// Amount of HNT to burn to DC
    #[arg(long)]
    amount: Decimal,

    /// Commit the burn
    #[arg(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        data_credits::
        let mut txn = BlockchainTxnTokenBurnV1 {
            fee: 0,
            payee: self.payee.to_vec(),
            amount: u64::from(self.amount),
            payer: keypair.public_key().into(),
            memo: u64::from(&self.memo),
            nonce: if let Some(nonce) = self.nonce {
                nonce
            } else {
                let account = accounts::get(&client, &keypair.public_key().to_string()).await?;
                account.speculative_nonce + 1
            },
            signature: Vec::new(),
        };

        txn.fee = if let Some(fee) = self.fee {
            fee
        } else {
            txn.txn_fee(&get_txn_fees(&client).await?)?
        };
        txn.signature = txn.sign(&keypair)?;

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_txn(&txn, &envelope, &status, &pending_url, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnTokenBurnV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    pending_url: &str,
) -> Result {
    let status_endpoint = pending_url.to_owned() + status_str(status);
    let table = json!({
        "payee": PublicKey::from_bytes(&txn.payee)?.to_string(),
        "amount": Hnt::from(txn.amount).to_f64(),
        "memo": Memo::from(txn.memo).to_string(),
        "fee": txn.fee,
        "nonce": txn.nonce,
        "hash": status_json(status),
        "txn": b64::encode_message(envelope)?,
        "status": status_endpoint
    });
    print_json(&table)    
}