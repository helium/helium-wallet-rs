use crate::{
    cmd::{api_url, get_password, load_wallet, Opts, OutputFormat},
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, Signer, TxnEnvelope, B58, B64},
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
            nonce: account.speculative_sec_nonce,
            fee: 0,
            signature: vec![],
        };

        let envelope = txn.sign(&keypair, Signer::Payer)?.in_envelope();
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
            if status.is_some() {
                ptable!(
                    ["Nonce", "Hash"],
                    [txn.nonce, status.as_ref().map_or("none", |s| &s.hash)]
                );
            }

            Ok(())
        }
        OutputFormat::Json => {
            let transfer = json!({
                    "payee": PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
                    "amount": txn.amount,
            });
            let table = if status.is_some() {
                json!({
                    "transfer": transfer,
                    "nonce": txn.nonce,
                    "hash": status.as_ref().map(|s| &s.hash),
                    "txn": envelope.to_b64()?,

                })
            } else {
                json!({
                    "transfer": transfer,
                    "txn": envelope.to_b64()?,
                })
            };
            println!("{}", serde_json::to_string_pretty(&table)?);
            Ok(())
        }
    }
}
