use super::{map_addresses, print_txn};
use crate::{
    cmd::{api_url, get_password, get_txn_fees, load_wallet, Opts},
    keypair::PublicKey,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, TxnStakingFee},
};
use helium_api::{BlockchainTxnOuiV1, Client};
use structopt::StructOpt;

/// Allocates an Organizational Unique Identifier (OUI) which
/// identifies endpoints for packets to sent to The transaction is not
/// submitted to the system unless the '--commit' option is given.
#[derive(Debug, StructOpt)]
pub struct Create {
    /// The address(es) of the router to send packets to
    #[structopt(long = "address", short = "a", number_of_values(1))]
    addresses: Vec<PublicKey>,

    /// Initial device membership filter in base64 encoded form
    #[structopt(long)]
    filter: String,

    /// Requested subnet size. Must be a value between 8 and 65,536
    /// and a power of two.
    #[structopt(long)]
    subnet_size: u32,

    /// Payer for the transaction (B58 address). If not specified the
    /// wallet is used.
    #[structopt(long)]
    payer: Option<PublicKey>,

    /// Commit the transaction to the API. If the staking server is
    /// used as the payer the transaction must first be submitted to
    /// the staking server for signing and the result submitted ot the
    /// API.
    #[structopt(long)]
    commit: bool,
}

impl Create {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let wallet_key = keypair.public_key();

        let api_client = Client::new_with_base_url(api_url(wallet.public_key.network));

        let mut txn = BlockchainTxnOuiV1 {
            addresses: map_addresses(self.addresses.clone(), |v| v.to_vec())?,
            owner: keypair.public_key().into(),
            payer: self.payer.as_ref().map_or(vec![], |v| v.into()),
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

        match self.payer.as_ref() {
            key if key == Some(&wallet_key) || key.is_none() => {
                // Payer is the wallet submit if ready to commit
                let status = if self.commit {
                    Some(api_client.submit_txn(&envelope)?)
                } else {
                    None
                };
                print_txn(&txn, &envelope, &status, opts.format)
            }
            _ => {
                // Payer is something else.
                // can't commit this transaction but we can display it
                print_txn(&txn, &envelope, &None, opts.format)
            }
        }
    }
}
