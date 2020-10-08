use crate::{
    cmd::{api_url, multisig::Artifact, print_json, Opts},
    keypair::PubKeyBin,
    result::Result,
    traits::{ToJson, TxnEnvelope},
};
use helium_api::{BlockchainTxnVarsV1, BlockchainVarV1, Client};
use std::{convert::TryInto, str::FromStr};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Commands for chain variables
pub enum Cmd {
    Current(Current),
    Create(Create),
}

#[derive(Debug, StructOpt)]
/// Lists current chain variables
pub struct Current {}

#[derive(Debug, StructOpt)]
/// Create a chain variable transaction
pub struct Create {
    /// Variables to set
    #[structopt(long, name = "name=value")]
    set: Vec<VarSet>,

    /// Variables to unset
    #[structopt(long, name = "name")]
    unset: Vec<String>,

    /// Variables to cancel
    #[structopt(long, name = "name")]
    cancel: Vec<String>,

    /// Signing keys to set
    #[structopt(long, name = "key")]
    key: Vec<PubKeyBin>,

    /// The nonce to use
    #[structopt(long, number_of_values(1))]
    nonce: Option<u32>,

    /// Return the encoded transaction for signing
    #[structopt(long)]
    txn: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Current(cmd) => cmd.run(opts),
            Cmd::Create(cmd) => cmd.run(opts),
        }
    }
}

impl Current {
    pub fn run(&self, _opts: Opts) -> Result {
        print_json(&get_vars()?)
    }
}

fn get_vars() -> Result<serde_json::Map<String, serde_json::Value>> {
    let client = Client::new_with_base_url(api_url());
    client.get_vars()
}

impl Create {
    pub fn run(&self, _opts: Opts) -> Result {
        let client = Client::new_with_base_url(api_url());
        let vars = client.get_vars()?;
        let mut txn = BlockchainTxnVarsV1 {
            version_predicate: 0,
            master_key: vec![],
            proof: vec![],
            key_proof: vec![],
            vars: self.set.iter().map(|v| v.0.clone()).collect(),
            nonce: self.nonce.unwrap_or_else(|| {
                vars.get("nonce")
                    .map_or(0, |v| v.as_u64().unwrap_or(0).try_into().unwrap())
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
            _ => return Err(format!("Invalid variable value {}", value.to_string()).into()),
        };
        Ok(VarSet(var))
    }
}
