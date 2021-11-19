use crate::{
    cmd::oui::*,
    traits::{TxnEnvelope, TxnFee, TxnSign, TxnStakingFee, B64},
};
use helium_api::{models::transactions::PendingTxnStatus, ouis};
use serde_json::json;
use structopt::StructOpt;

/// Updates an organizational OUI. The transaction is not
/// submitted to the system unless the '--commit' option is given.
#[derive(Debug, StructOpt)]
pub enum Update {
    /// The address(es) of the router to send packets to. This will overwrite any previous
    /// router(s)
    Routers(Routers),
    /// Create new or update Xor filter by hand. This is normally done by the OUI operator,
    /// but may be done by hand here.
    Xor(Xor),
    /// Requested additional subnet size. Must be a value between 8 and 65,536
    /// and a power of two.
    RequestSubset(RequestSubnet),
}

#[derive(Debug, StructOpt)]
pub struct Routers {
    /// OUI to update
    #[structopt(required = true, long)]
    pub oui: u32,
    /// The address(es) of the router to send packets to
    #[structopt(required = true, long = "address", short = "a", number_of_values(1))]
    pub addresses: Vec<PublicKey>,
    /// Which OUI nonce this transaction has
    #[structopt(long)]
    pub nonce: Option<u64>,
    /// Commit the transaction to the API
    #[structopt(long)]
    pub commit: bool,
}

#[derive(Debug, StructOpt)]
pub enum Xor {
    /// If less than 5 filters have been defined,
    /// you can create an additional Xor
    New(xor::New),
    /// Overwrite an existing Xor filter
    Update(xor::Update),
}

pub mod xor {
    use super::super::StructOpt;
    #[derive(Debug, StructOpt)]
    /// Update an already defined Xor
    pub struct Update {
        /// OUI to update
        #[structopt(required = true, long)]
        pub oui: u32,
        /// select which Xor to update
        #[structopt(required = true, long)]
        pub index: u32,
        /// 100kb or less
        #[structopt(required = true, long)]
        pub filter: String,
        /// Which OUI nonce this transaction has
        #[structopt(long)]
        pub nonce: Option<u64>,
        /// Commit the transaction to the API
        #[structopt(long)]
        pub commit: bool,
    }
    #[derive(Debug, StructOpt)]
    pub struct New {
        /// OUI to update
        #[structopt(required = true, long)]
        pub oui: u32,
        /// 100kb or less
        #[structopt(required = true, long)]
        pub filter: String,
        /// Which OUI nonce this transaction has
        #[structopt(long)]
        pub nonce: Option<u64>,
        /// Commit the transaction to the API
        #[structopt(long)]
        pub commit: bool,
    }
}

#[derive(Debug, StructOpt)]
pub struct RequestSubnet {
    /// OUI to update
    #[structopt(required = true, long)]
    pub oui: u32,
    #[structopt(required = true, long)]
    pub size: u32,
    /// Which OUI nonce this transaction has
    #[structopt(long)]
    pub nonce: Option<u64>,
    /// Commit the transaction to the API
    #[structopt(long)]
    pub commit: bool,
}

impl Update {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let client = new_client(api_url(wallet.public_key.network));

        let (oui, commit, nonce, update) = match self {
            Update::Routers(routers) => (
                routers.oui,
                routers.commit,
                routers.nonce,
                blockchain_txn_routing_v1::Update::UpdateRouters(UpdateRouters {
                    router_addresses: map_addresses(routers.addresses.clone(), |v| v.to_vec())?,
                }),
            ),
            Update::Xor(xor) => match xor {
                Xor::New(filter) => (
                    filter.oui,
                    filter.commit,
                    filter.nonce,
                    blockchain_txn_routing_v1::Update::NewXor(base64::decode(&filter.filter)?),
                ),
                Xor::Update(update) => (
                    update.oui,
                    update.commit,
                    update.nonce,
                    blockchain_txn_routing_v1::Update::UpdateXor(UpdateXor {
                        index: update.index,
                        filter: base64::decode(&update.filter)?,
                    }),
                ),
            },
            Update::RequestSubset(size) => (
                size.oui,
                size.commit,
                size.nonce,
                blockchain_txn_routing_v1::Update::RequestSubnet(size.size),
            ),
        };

        let mut txn = BlockchainTxnRoutingV1 {
            // the type in the proto diverges from the more common u64
            oui: oui as u32,
            owner: keypair.public_key().into(),
            fee: 0,
            signature: vec![],
            staking_fee: 0,
            update: Some(update),
            nonce: if let Some(nonce) = nonce {
                nonce
            } else {
                ouis::get(&client, oui.into()).await?.nonce + 1
            },
        };
        let fees = get_txn_fees(&client).await?;
        txn.fee = txn.txn_fee(&fees)?;
        txn.staking_fee = txn.txn_staking_fee(&fees)?;
        txn.signature = txn.sign(&keypair)?;
        let envelope = txn.in_envelope();

        let status = maybe_submit_txn(commit, &client, &envelope).await?;
        print_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnRoutingV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let update = match txn.update.as_ref().unwrap() {
        blockchain_txn_routing_v1::Update::UpdateRouters(txn) => {
            let mut str = String::from("Routing ");
            let addr = map_addresses(txn.router_addresses.clone(), |v| v.to_string())?;
            str.extend(addr);
            str
        }
        blockchain_txn_routing_v1::Update::NewXor(_) => "NewXor".into(),
        blockchain_txn_routing_v1::Update::UpdateXor(txn) => format!("Update Xor {}", txn.index),
        blockchain_txn_routing_v1::Update::RequestSubnet(size) => {
            format!("Request subnet of size {}", size)
        }
    };

    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Last OUI", txn.oui],
                ["Update", update],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "last_oui": txn.oui,
                "update": update,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
        }
    }
}
