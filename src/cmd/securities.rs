use crate::{
    cmd::*,
    keypair::PublicKey,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, B64},
};
use helium_api::accounts;

#[derive(Debug, StructOpt)]
/// Work with security tokens
pub enum Cmd {
    Transfer(Transfer),
}

#[derive(Debug, StructOpt)]
/// Transfer security tokens to the given target account
pub struct Transfer {
    /// The address of the recipient of the security tokens
    payee: PublicKey,

    /// The number of security tokens to transfer
    amount: Hst,

    /// Commit the transfter to the API
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Transfer(cmd) => cmd.run(opts).await,
        }
    }
}

impl Transfer {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;

        let client = new_client(api_url(wallet.public_key.network));

        let keypair = wallet.decrypt(password.as_bytes())?;
        let account = accounts::get(&client, &keypair.public_key().to_string()).await?;

        let mut txn = BlockchainTxnSecurityExchangeV1 {
            payer: keypair.public_key().into(),
            payee: self.payee.to_vec(),
            amount: u64::from(self.amount),
            nonce: account.speculative_sec_nonce + 1,
            fee: 0,
            signature: vec![],
        };
        txn.fee = txn.txn_fee(&get_txn_fees(&client).await?)?;
        txn.signature = txn.sign(&keypair)?;

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnSecurityExchangeV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let payee = PublicKey::from_bytes(&txn.payee)?.to_string();
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Payee", payee],
                ["Amount (HST)", Hst::from(txn.amount)],
                ["Fee (DC)", txn.fee],
                ["Nonce", txn.nonce],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "payee": payee,
                "amount": txn.amount,
                    "fee": txn.fee,
             "nonce": txn.nonce,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
        }
    }
}
