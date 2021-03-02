use crate::{
    cmd::{
        api_url, get_password, get_txn_fees, load_wallet, print_footer, print_json, status_json,
        status_str, Opts, OutputFormat,
    },
    keypair::PublicKey,
    result::{anyhow, Result},
    traits::{TxnEnvelope, TxnFee, TxnSign, TxnStakingFee, B64},
};
use helium_api::{
    blockchain_txn_routing_v1::Update as UpdateTxn, BlockchainTxn, BlockchainTxnOuiV1,
    BlockchainTxnRoutingV1, Client, PendingTxnStatus, Txn, UpdateRouters, UpdateXor,
};
use serde_json::json;
use std::convert::TryInto;
use structopt::StructOpt;

/// Create or update an OUI
#[derive(Debug, StructOpt)]
pub enum Cmd {
    Create(Create),
    Submit(Submit),
    Update(Update),
}

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

/// Updates an organizational OUI. The transaction is not
/// submitted to the system unless the '--commit' option is given.
#[derive(Debug, StructOpt)]
pub enum Update {
    /// The address(es) of the router to send packets to. This will overwrite any previous
    /// routers
    Routers(update::Routers),
    /// If less than 5 filters have been defined,
    /// you can create an additional Xor
    NewXor(update::NewXor),
    UpdateXor(update::UpdateXor),
    /// Requested additional subnet size. Must be a value between 8 and 65,536
    /// and a power of two.
    RequestSubset(update::RequestSubet),
}

mod update {
    use super::{PublicKey, StructOpt};
    #[derive(Debug, StructOpt)]
    pub struct Routers {
        /// OUI to update
        #[structopt(long)]
        pub oui: u32,
        /// The address(es) of the router to send packets to
        #[structopt(long = "address", short = "a", number_of_values(1))]
        pub addresses: Vec<PublicKey>,
        /// Which OUI nonce this transaction has
        #[structopt(long)]
        pub nonce: Option<u64>,
        /// Commit the transaction to the API
        #[structopt(long)]
        pub commit: bool,
    }
    #[derive(Debug, StructOpt)]
    /// Update an already defined Xor
    pub struct UpdateXor {
        /// OUI to update
        #[structopt(long)]
        pub oui: u32,
        /// select which Xor to update
        pub index: u32,
        /// 100kb or less
        pub filter: String,
        /// Which OUI nonce this transaction has
        #[structopt(long)]
        pub nonce: Option<u64>,
        /// Commit the transaction to the API
        #[structopt(long)]
        pub commit: bool,
    }
    #[derive(Debug, StructOpt)]
    pub struct NewXor {
        /// OUI to update
        #[structopt(long)]
        pub oui: u32,
        /// 100kb or less
        pub filter: String,
        /// Which OUI nonce this transaction has
        #[structopt(long)]
        pub nonce: Option<u64>,
        /// Commit the transaction to the API
        #[structopt(long)]
        pub commit: bool,
    }
    #[derive(Debug, StructOpt)]
    pub struct RequestSubet {
        /// OUI to update
        #[structopt(long)]
        pub oui: u32,
        #[structopt(long)]
        pub size: u32,
        /// Which OUI nonce this transaction has
        #[structopt(long)]
        pub nonce: Option<u64>,
        /// Commit the transaction to the API
        #[structopt(long)]
        pub commit: bool,
    }
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Create(cmd) => cmd.run(opts),
            Cmd::Submit(cmd) => cmd.run(opts),
            Cmd::Update(cmd) => cmd.run(opts),
        }
    }
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

impl Submit {
    pub fn run(&self, opts: Opts) -> Result {
        let envelope = BlockchainTxn::from_b64(&self.transaction)?;
        if let Some(Txn::Oui(t)) = envelope.txn.clone() {
            let api_url = api_url(PublicKey::from_bytes(&t.owner)?.network);
            let api_client = helium_api::Client::new_with_base_url(api_url);
            let status = if self.commit {
                Some(api_client.submit_txn(&envelope)?)
            } else {
                None
            };
            print_txn(&t, &envelope, &status, opts.format)
        } else {
            Err(anyhow!("Invalid OUI transaction"))
        }
    }
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
            Update::NewXor(filter) => (
                filter.oui,
                filter.commit,
                filter.nonce,
                helium_api::blockchain_txn_routing_v1::Update::NewXor(base64::decode(
                    &filter.filter,
                )?),
            ),
            Update::UpdateXor(update) => (
                update.oui,
                update.commit,
                update.nonce,
                helium_api::blockchain_txn_routing_v1::Update::UpdateXor(UpdateXor {
                    index: update.index,
                    filter: base64::decode(&update.filter)?,
                }),
            ),
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
            Some(api_client.submit_txn(&envelope)?)
        } else {
            None
        };
        print_update_txn(&txn, &envelope, &status, opts.format)
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
                ["Requested OUI", txn.oui],
                ["Reqeuested Subnet Size", txn.requested_subnet_size],
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
                "requested_oui": txn.oui,
                "addresses": map_addresses(txn.addresses.clone(), |v| v.to_string())?,
                "requested_subnet_size": txn.requested_subnet_size,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });

            print_json(&table)
        }
    }
}

fn map_addresses<F, R>(addresses: Vec<impl TryInto<PublicKey>>, f: F) -> Result<Vec<R>>
where
    F: Fn(PublicKey) -> R,
{
    let results: Result<Vec<R>> = addresses
        .into_iter()
        .map(|v| match v.try_into() {
            Ok(public_key) => Ok(f(public_key)),
            Err(_err) => Err(anyhow!("failed to convert to public key")),
        })
        .collect();
    results
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
                ["oui", txn.oui],
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
