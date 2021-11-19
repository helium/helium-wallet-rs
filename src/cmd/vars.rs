use crate::{
    cmd::multisig::Artifact,
    cmd::*,
    keypair::{Network, PublicKey},
    result::Result,
    traits::{ToJson, TxnEnvelope},
};
use helium_api::vars;
use std::{convert::TryInto, str::FromStr};

#[derive(Debug, StructOpt)]
/// Commands for chain variables
pub enum Cmd {
    Current(Current),
    Create(Create),
}

#[derive(Debug, StructOpt)]
/// Lists current chain variables
pub struct Current {
    /// The network to get the variables for (mainnet/testnet). Defaults to the
    /// network associated with the active wallet.
    #[structopt(long)]
    network: Option<Network>,
}

#[derive(Debug, StructOpt)]
/// Create a chain variable transaction
pub struct Create {
    /// Set of Variables to set
    #[structopt(long, name = "set_name=value")]
    set: Vec<VarSet>,

    /// Variable to unset
    #[structopt(long, name = "unset_name")]
    unset: Vec<String>,

    /// Variable to cancel
    #[structopt(long, name = "cancel_name")]
    cancel: Vec<String>,

    /// Signing keys to set
    #[structopt(long, name = "key")]
    key: Vec<PublicKey>,

    /// The nonce to use
    #[structopt(long, number_of_values(1))]
    nonce: Option<u32>,

    /// Return the encoded transaction for signing
    #[structopt(long)]
    txn: bool,

    /// The network to create the variables for (mainnet/testnet). Defaults to
    /// the network associated with the active wallet.
    #[structopt(long)]
    network: Option<Network>,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Current(cmd) => cmd.run(opts).await,
            Cmd::Create(cmd) => cmd.run(opts).await,
        }
    }
}

impl Current {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;
        let network = self.network.unwrap_or(wallet.public_key.network);
        let client = new_client(api_url(network));
        let vars = vars::get(&client).await?;
        print_json(&vars)
    }
}

impl Create {
    pub async fn run(&self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;
        let network = self.network.unwrap_or(wallet.public_key.network);
        let client = new_client(api_url(network));
        let vars = vars::get(&client).await?;

        let mut txn = BlockchainTxnVarsV1 {
            version_predicate: 0,
            master_key: vec![],
            proof: vec![],
            key_proof: vec![],
            vars: self.set.iter().map(|v| v.0.clone()).collect(),
            nonce: self.nonce.unwrap_or_else(|| {
                vars.get("nonce").map_or(0, |v| {
                    let result: u32 = v.as_u64().unwrap_or(0).try_into().unwrap();
                    result + 1
                })
            }),
            unsets: self.unset.iter().map(|v| v.as_bytes().to_vec()).collect(),
            cancels: self.cancel.iter().map(|v| v.as_bytes().to_vec()).collect(),
            multi_key_proofs: vec![],
            multi_proofs: vec![],
            multi_keys: self.key.iter().map(|v| v.to_vec()).collect(),
        };

        txn.multi_keys.dedup_by(|a, b| a == b);

        if self.txn {
            print_json(&Artifact::from_txn(&txn.in_envelope())?)
        } else {
            print_json(&txn.to_json()?)
        }
    }
}

#[derive(Debug)]
struct VarSet(BlockchainVarV1);

impl FromStr for VarSet {
    type Err = Box<dyn std::error::Error>;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let pos = s
            .find('=')
            .ok_or_else(|| format!("invalid KEY=value: missing `=`  in `{}`", s))?;
        let name = s[..pos].to_string();
        let value: serde_json::Value = s[pos + 1..].parse()?;
        let var = match value {
            serde_json::Value::Number(n) if n.is_i64() => BlockchainVarV1 {
                name,
                r#type: "int".to_string(),
                value: n.to_string().as_bytes().to_vec(),
            },
            serde_json::Value::Number(n) if n.is_f64() => BlockchainVarV1 {
                name,
                r#type: "float".to_string(),
                value: n.to_string().as_bytes().to_vec(),
            },
            serde_json::Value::String(s) => BlockchainVarV1 {
                name,
                r#type: "string".to_string(),
                value: s.as_bytes().to_vec(),
            },
            _ => BlockchainVarV1 {
                name,
                r#type: "atom".to_string(),
                value: s.as_bytes().to_vec(),
            },
        };
        Ok(VarSet(var))
    }
}
