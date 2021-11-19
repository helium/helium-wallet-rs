use crate::{
    cmd::oui::*,
    traits::{TxnEnvelope, TxnFee, TxnSign, TxnStakingFee},
};
use helium_api::ouis;
use structopt::StructOpt;

/// Allocates an Organizational Unique Identifier (OUI) which
/// identifies endpoints for packets to sent to The transaction is not
/// submitted to the system unless the '--commit' option is given.
#[derive(Debug, StructOpt)]
pub struct Create {
    /// The address(es) of the router to send packets to
    #[structopt(long = "address", short = "a", number_of_values(1))]
    addresses: Vec<PublicKey>,

    /// Optionally indicate last OUI. Wallet will determine
    /// this using API otherwise.
    #[structopt(long)]
    last_oui: Option<u64>,

    /// Initial device membership filter in base64 encoded form.
    /// Dummy filter default is given, but OUI addresses may
    /// overwrite this at any time.
    #[structopt(
        long,
        default_value = "wVwCiewtCpEKAAAAAAAAAAAAcCK3fwAAAAAAAAAAAABI7IQOAHAAAAAAAAAAAAAAAQAAADBlAAAAAAAAAAAAADEAAAA2AAAAOgAAAA"
    )]
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
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let wallet_key = keypair.public_key();

        let client = new_client(api_url(wallet.public_key.network));

        let oui = if let Some(oui) = self.last_oui {
            oui
        } else {
            ouis::last(&client).await?.oui
        };

        let mut txn = BlockchainTxnOuiV1 {
            addresses: map_addresses(self.addresses.clone(), |v| v.to_vec())?,
            owner: keypair.public_key().into(),
            payer: self.payer.as_ref().map_or(vec![], |v| v.into()),
            oui,
            fee: 0,
            staking_fee: 1,
            owner_signature: vec![],
            payer_signature: vec![],
            requested_subnet_size: self.subnet_size,
            filter: base64::decode(&self.filter)?,
        };

        let fees = &get_txn_fees(&client).await?;

        txn.fee = txn.txn_fee(fees)?;
        txn.staking_fee = txn.txn_staking_fee(fees)?;

        txn.owner_signature = txn.sign(&keypair)?;
        let envelope = txn.in_envelope();

        match self.payer.as_ref() {
            key if key == Some(wallet_key) || key.is_none() => {
                // Payer is the wallet submit if ready to commit
                let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
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
                ["Previous OUI", txn.oui],
                ["Requested Subnet Size", txn.requested_subnet_size],
                [
                    "Addresses",
                    map_addresses(txn.addresses.clone(), |v| v.to_string())?.join("\n")
                ],
                ["Hash", status_str(status)]
            );

            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "previous_oui": txn.oui,
                "addresses": map_addresses(txn.addresses.clone(), |v| v.to_string())?,
                "requested_subnet_size": txn.requested_subnet_size,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });

            print_json(&table)
        }
    }
}
