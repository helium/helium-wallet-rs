use crate::{
    cmd::{
        api_url, get_password, get_txn_fees, load_wallet, print_footer, print_json, status_json,
        status_str, Opts, OutputFormat,
    },
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, TxnEnvelope, TxnFee, B58, B64},
};
use helium_api::{BlockchainTxn, BlockchainTxnTokenBurnV1, Client, Hnt, PendingTxnStatus};
use serde_json::json;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Burn HNT to Data Credits (DC) from this wallet to given payees wallet.
pub struct Cmd {
    /// Account address to send the resulting DC to.
    #[structopt(long)]
    payee: String,

    /// Memo field to include. Provide as a base64 encoded string
    #[structopt(long)]
    memo: Option<String>,

    /// Amount of HNT to burn to DC
    #[structopt(long)]
    amount: Hnt,

    /// Commit the payment to the API
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;

        let client = Client::new_with_base_url(api_url());

        let keypair = wallet.decrypt(password.as_bytes())?;
        let account = client.get_account(&keypair.public.to_b58()?)?;
        let memo = match &self.memo {
            None => 0,
            Some(s) => u64::from_b64(&s)?,
        };

        let mut txn = BlockchainTxnTokenBurnV1 {
            fee: 0,
            payee: PubKeyBin::from_b58(&self.payee)?.into(),
            amount: self.amount.to_bones(),
            payer: keypair.pubkey_bin().into(),
            memo,
            nonce: account.speculative_nonce + 1,
            signature: Vec::new(),
        };
        txn.fee = txn.txn_fee(&get_txn_fees(&client)?)?;
        txn.signature = txn.sign(&keypair)?;
        let envelope = txn.in_envelope();
        let status = if self.commit {
            Some(client.submit_txn(&envelope)?)
        } else {
            None
        };
        print_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnTokenBurnV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Payee", PubKeyBin::from_vec(&txn.payee).to_b58().unwrap()],
                ["Memo", txn.memo.to_b64()?],
                ["Amount", Hnt::from_bones(txn.amount)],
                ["Fee", txn.fee],
                ["Nonce", txn.nonce],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "payee": PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
                "amount": Hnt::from_bones(txn.amount),
                "memo": txn.memo.to_b64()?,
                "fee": txn.fee,
                "nonce": txn.nonce,
                "hash": status_json(status),
                "txn": envelope.to_b64()?
            });
            print_json(&table)
        }
    }
}
