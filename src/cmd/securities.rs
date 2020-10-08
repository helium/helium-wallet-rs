use crate::{
    cmd::{
        api_url, get_password, get_txn_fees, load_wallet, print_footer, print_json, status_json,
        status_str, Opts, OutputFormat,
    },
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, TxnEnvelope, TxnFee, B58, B64},
};
use helium_api::{BlockchainTxn, BlockchainTxnSecurityExchangeV1, Client, PendingTxnStatus};
use serde_json::json;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Work with security tokens
pub enum Cmd {
    Transfer(Transfer),
}

#[derive(Debug, StructOpt)]
/// Transfer security tokens to the given target account
pub struct Transfer {
    /// The address of the recipient of the security tokens
    payee: String,

    /// The number of security tokens to transfer
    amount: u64,

    /// Commit the transfter to the API
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Transfer(cmd) => cmd.run(opts),
        }
    }
}

impl Transfer {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;

        let client = Client::new_with_base_url(api_url());

        let keypair = wallet.decrypt(password.as_bytes())?;
        let account = client.get_account(&keypair.public.to_b58()?)?;

        let mut txn = BlockchainTxnSecurityExchangeV1 {
            payer: keypair.pubkey_bin().into(),
            payee: PubKeyBin::from_b58(&self.payee)?.into(),
            amount: self.amount,
            nonce: account.speculative_sec_nonce + 1,
            fee: 0,
            signature: vec![],
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
    txn: &BlockchainTxnSecurityExchangeV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Payee", "Amount"],
                [
                    PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
                    txn.amount
                ]
            );
            ptable!(
                ["Key", "Value"],
                ["Nonce", txn.nonce],
                ["Hash", status_str(status)]
            );

            print_footer(status)
        }
        OutputFormat::Json => {
            let transfer = json!({
                    "payee": PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
                    "amount": txn.amount,
            });
            let table = json!({
                "transfer": transfer,
                "nonce": txn.nonce,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
        }
    }
}
