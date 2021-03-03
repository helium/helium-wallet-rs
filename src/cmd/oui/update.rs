use super::map_addresses;
use crate::{
    cmd::{
        api_url, get_password, get_txn_fees, load_wallet, print_footer, print_json, status_json,
        status_str, Opts, OutputFormat,
    },
    keypair::PublicKey,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, TxnStakingFee, B64},
};
use helium_api::{
    blockchain_txn_routing_v1::Update as UpdateTxn, BlockchainTxn, BlockchainTxnRoutingV1, Client,
    PendingTxnStatus, UpdateRouters, UpdateXor,
};
use serde_json::json;
use structopt::StructOpt;

/// Updates an organizational OUI. The transaction is not
/// submitted to the system unless the '--commit' option is given.
#[derive(Debug, StructOpt)]
pub enum Update {
    /// The address(es) of the router to send packets to. This will overwrite any previous
    /// routers
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
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let api_client = Client::new_with_base_url(api_url(wallet.public_key.network));

        let (oui, commit, nonce, update) = match self {
            Update::Routers(routers) => (
                routers.oui,
                routers.commit,
                routers.nonce,
                helium_api::blockchain_txn_routing_v1::Update::UpdateRouters(UpdateRouters {
                    router_addresses: map_addresses(routers.addresses.clone(), |v| v.to_vec())?,
                }),
            ),
            Update::Xor(xor) => match xor {
                Xor::New(filter) => (
                    filter.oui,
                    filter.commit,
                    filter.nonce,
                    helium_api::blockchain_txn_routing_v1::Update::NewXor(base64::decode(
                        &filter.filter,
                    )?),
                ),
                Xor::Update(update) => (
                    update.oui,
                    update.commit,
                    update.nonce,
                    helium_api::blockchain_txn_routing_v1::Update::UpdateXor(UpdateXor {
                        index: update.index,
                        filter: base64::decode(&update.filter)?,
                    }),
                ),
            },
            Update::RequestSubset(size) => (
                size.oui,
                size.commit,
                size.nonce,
                helium_api::blockchain_txn_routing_v1::Update::RequestSubnet(size.size),
            ),
        };

        let mut txn = BlockchainTxnRoutingV1 {
            oui,
            owner: keypair.public_key().into(),
            fee: 0,
            signature: vec![],
            staking_fee: 0,
            update: Some(update),
            nonce: if let Some(nonce) = nonce {
                nonce
            } else {
                //TODO: fetch nonce from API
                0
            },
        };
        txn.fee = txn.txn_fee(&get_txn_fees(&api_client)?)?;
        txn.staking_fee = txn.txn_staking_fee(&get_txn_fees(&api_client)?)?;
        txn.signature = txn.sign(&keypair)?;
        let envelope = txn.in_envelope();

        let status = if commit {
            let response = api_client.submit_txn(&envelope);
            println!("{:?}", response);
            Some(response.unwrap())
        } else {
            None
        };
        print_update_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_update_txn(
    txn: &BlockchainTxnRoutingV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let update = match txn.update.as_ref().unwrap() {
        UpdateTxn::UpdateRouters(txn) => {
            let mut str = String::from("Routing ");
            let addr = map_addresses(txn.router_addresses.clone(), |v| v.to_string())?;
            str.extend(addr);
            str
        }
        UpdateTxn::NewXor(_) => "NewXor".into(),
        UpdateTxn::UpdateXor(txn) => format!("Update Xor {}", txn.index),
        UpdateTxn::RequestSubnet(size) => format!("Request subnet of size {}", size),
    };

    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["OUI", txn.oui],
                ["Update", update],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "oui": txn.oui,
                "Update": update,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
        }
    }
}
