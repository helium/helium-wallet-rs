use crate::{
    cmd::*,
    keypair::{Keypair, PublicKey},
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, B64},
};
use helium_api::accounts;
use serde_json::json;

#[derive(Debug, StructOpt)]
/// Create or Redeem from an HTLC address
pub enum Cmd {
    Create(Create),
    Redeem(Redeem),
}

#[derive(Debug, StructOpt)]
/// Creates a new HTLC address with a specified hashlock and timelock (in block height), and transfers a value of tokens to it.
/// The transaction is not submitted to the system unless the '--commit' option is given.
pub struct Create {
    /// The address of the intended payee for this HTLC
    payee: PublicKey,

    /// Number of hnt to send
    #[structopt(long)]
    hnt: Hnt,

    /// A hex encoded SHA256 digest of a secret value (called a preimage) that locks this contract
    #[structopt(long = "hashlock")]
    hashlock: String,

    /// A specific blockheight after which the payer (you) can redeem their tokens
    #[structopt(long = "timelock")]
    timelock: u64,

    /// Commit the payment to the API
    #[structopt(long)]
    commit: bool,
}

#[derive(Debug, StructOpt)]
/// Redeem the balance from an HTLC address with the specified preimage for the hashlock
pub struct Redeem {
    /// Address of the HTLC contract to redeem from
    address: PublicKey,

    /// The preimage used to create the hashlock for this contract address
    #[structopt(short = "p", long = "preimage")]
    preimage: String,

    /// Commit the payment to the API
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Create(cmd) => cmd.run(opts).await,
            Cmd::Redeem(cmd) => cmd.run(opts).await,
        }
    }
}

impl Create {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let client = new_client(api_url(wallet.public_key.network));

        let keypair = wallet.decrypt(password.as_bytes())?;
        let wallet_address = keypair.public_key();
        let account = accounts::get(&client, &wallet_address.to_string()).await?;
        let address = Keypair::generate(wallet_address.key_tag());

        let mut txn = BlockchainTxnCreateHtlcV1 {
            amount: u64::from(self.hnt),
            fee: 0,
            payee: self.payee.to_vec(),
            payer: wallet_address.to_vec(),
            address: address.public_key().to_vec(),
            hashlock: hex::decode(self.hashlock.clone()).unwrap(),
            timelock: self.timelock,
            nonce: account.speculative_nonce + 1,
            signature: Vec::new(),
        };
        txn.fee = txn.txn_fee(&get_txn_fees(&client).await?)?;
        txn.signature = txn.sign(&keypair)?;
        let envelope = txn.in_envelope();

        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_create_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_create_txn(
    txn: &BlockchainTxnCreateHtlcV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Address", PublicKey::from_bytes(&txn.address)?.to_string()],
                ["Payee", PublicKey::from_bytes(&txn.payee)?.to_string()],
                ["Amount (HNT)", Hnt::from(txn.amount)],
                ["Fee (DC)", txn.fee],
                ["Hashlock", hex::encode(&txn.hashlock)],
                ["Timelock", txn.timelock],
                ["Nonce", txn.nonce],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "address": PublicKey::from_bytes(&txn.address)?.to_string(),
                "payee": PublicKey::from_bytes(&txn.payee)?.to_string(),
                "amount": txn.amount,
                "fee": txn.fee,
                "hashlock": hex::encode(&txn.hashlock),
                "timelock": txn.timelock,
                "nonce": txn.nonce,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
        }
    }
}

impl Redeem {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let client = new_client(api_url(wallet.public_key.network));

        let mut txn = BlockchainTxnRedeemHtlcV1 {
            fee: 0,
            payee: keypair.public_key().to_vec(),
            address: self.address.to_vec(),
            preimage: self.preimage.clone().into_bytes(),
            signature: Vec::new(),
        };
        txn.fee = txn.txn_fee(&get_txn_fees(&client).await?)?;
        txn.signature = txn.sign(&keypair)?;

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_redeem_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_redeem_txn(
    txn: &BlockchainTxnRedeemHtlcV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Payee", PublicKey::from_bytes(&txn.payee)?.to_string()],
                ["Address", PublicKey::from_bytes(&txn.address)?.to_string()],
                ["Preimage", std::str::from_utf8(&txn.preimage)?],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "address": PublicKey::from_bytes(&txn.address)?.to_string(),
                "payee": PublicKey::from_bytes(&txn.payee)?.to_string(),
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
        }
    }
}
