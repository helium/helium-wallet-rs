use crate::{
    cmd::{
        api_url, get_password, get_payer, get_txn_fees, load_wallet, print_footer, print_json,
        status_json, status_str, Opts, OutputFormat,
    },
    keypair::PubKeyBin,
    result::Result,
    staking,
    traits::{Sign, TxnEnvelope, TxnFee, TxnStakingFee, B64},
};
use helium_api::{BlockchainTxn, BlockchainTxnOuiV1, Client, PendingTxnStatus, Txn};
use serde_json::json;
use structopt::StructOpt;

/// Create or update an OUI
#[derive(Debug, StructOpt)]
pub enum Cmd {
    Create(Create),
    Submit(Submit),
}

/// Allocates an Organizational Unique Identifier (OUI) which
/// identifies endpoints for packets to sent to The transaction is not
/// submitted to the system unless the '--commit' option is given.
#[derive(Debug, StructOpt)]
pub struct Create {
    /// The address(es) of the router to send packets to
    #[structopt(long = "address", short = "a", number_of_values(1))]
    addresses: Vec<PubKeyBin>,

    /// Initial device membership filter in base64 encoded form
    #[structopt(long)]
    filter: String,

    /// Requested subnet size. Must be a value between 8 and 65,536
    /// and a power of two.
    #[structopt(long)]
    subnet_size: u32,

    /// Payer for the transaction (B58 address). If not specified the
    /// wallet is used. If "staking" is used the Helium staking server
    /// is used as the payer.
    #[structopt(long)]
    payer: Option<String>,

    /// Commit the transaction to the API. If the staking server is
    /// used as the payer the transaction must first be submitted to
    /// the staking server for signing and the result submitted ot the
    /// API.
    #[structopt(long)]
    commit: bool,
}

/// Submits a given base64 oui transaction to the API. This command
/// can be used when this wallet is not the payer of the oui
/// transaction.
#[derive(Debug, StructOpt)]
pub struct Submit {
    /// Base64 encoded transaction to submit.
    #[structopt(name = "TRANSACTION")]
    transaction: String,

    /// Commit the payment to the API. If the staking server is used
    /// as the payer the transaction is first submitted to the staking
    /// server for signing and the result submitted ot the API.
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Create(cmd) => cmd.run(opts),
            Cmd::Submit(cmd) => cmd.run(opts),
        }
    }
}

impl Create {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let api_client = Client::new_with_base_url(api_url());
        let staking_client = staking::Client::default();

        let staking_key = staking_client.address()?;
        let wallet_key = keypair.pubkey_bin();

        let payer = get_payer(staking_key, &self.payer)?;

        let mut txn = BlockchainTxnOuiV1 {
            addresses: self
                .addresses
                .clone()
                .into_iter()
                .map(|s| s.to_vec())
                .collect(),
            owner: keypair.pubkey_bin().into(),
            payer: payer.map_or(vec![], |v| v.to_vec()),
            oui: api_client.get_last_oui()?,
            fee: 0,
            staking_fee: 1,
            owner_signature: vec![],
            payer_signature: vec![],
            requested_subnet_size: self.subnet_size,
            filter: base64::decode(&self.filter)?,
        };
        txn.fee = txn.txn_fee(&get_txn_fees(&api_client)?)?;
        txn.staking_fee = txn.txn_staking_fee(&get_txn_fees(&api_client)?)?;
        txn.owner_signature = txn.sign(&keypair)?;
        let envelope = txn.in_envelope();

        match payer {
            key if key == Some(wallet_key) || key.is_none() => {
                // Payer is the wallet submit if ready to commit
                let status = if self.commit {
                    Some(api_client.submit_txn(&envelope)?)
                } else {
                    None
                };
                print_txn(&txn, &envelope, &status, opts.format)
            }
            _ => {
                // Payer is either staking server or something else.
                // can't commit this transaction but we can display it
                print_txn(&txn, &envelope, &None, opts.format)
            }
        }
    }
}

impl Submit {
    pub fn run(&self, opts: Opts) -> Result {
        let envelope = BlockchainTxn::from_b64(&self.transaction)?;
        if let Some(Txn::Oui(t)) = envelope.txn.clone() {
            let api_client = helium_api::Client::new_with_base_url(api_url());
            let status = if self.commit {
                Some(api_client.submit_txn(&envelope)?)
            } else {
                None
            };
            print_txn(&t, &envelope, &status, opts.format)
        } else {
            Err("Invalid OUI transaction".into())
        }
    }
}

fn print_txn(
    txn: &BlockchainTxnOuiV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Requested OUI", txn.oui + 1],
                ["Reqeuested Subnet Size", txn.requested_subnet_size],
                [
                    "Addresses",
                    txn.addresses
                        .clone()
                        .into_iter()
                        .map(|v| PubKeyBin::from_vec(&v).to_string())
                        .collect::<Vec<String>>()
                        .join("\n")
                ],
                ["Hash", status_str(status)]
            );

            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "requested_oui": txn.oui + 1,
                "addresses": txn.addresses
                    .clone()
                    .into_iter()
                    .map(|v| PubKeyBin::from_vec(&v).to_string())
                    .collect::<Vec<String>>(),
                "requested_subnet_size": txn.requested_subnet_size,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });

            print_json(&table)
        }
    }
}
