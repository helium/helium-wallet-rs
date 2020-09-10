use crate::{
    cmd::{
        api_url, get_password, load_wallet, print_footer, print_json, print_table, status_json,
        status_str, Opts, OutputFormat,
    },
    result::Result,
    traits::{Sign, Signer, TxnEnvelope, B64},
};
use helium_api::{BlockchainTxn, BlockchainTxnVarsV1, BlockchainVarV1, Client, PendingTxnStatus};
use prettytable::Table;
use std::str::FromStr;
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
    #[structopt(long, default_value = "0")]
    version_predicate: u32,

    /// Variables to set
    #[structopt(long, name = "name=value", number_of_values(1))]
    set: Vec<VarSet>,

    /// Variables to unset
    #[structopt(long, name = "name", number_of_values(1))]
    unset: Vec<String>,

    /// The nonce to use
    #[structopt(long, number_of_values(1))]
    nonce: u32,

    /// Commit the variables to the API
    #[structopt(long)]
    commit: bool,
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
    pub fn run(&self, opts: Opts) -> Result {
        let client = Client::new_with_base_url(api_url());
        let vars = client.get_vars()?;
        print_vars(&vars, opts.format)
    }
}

impl Create {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let client = Client::new_with_base_url(api_url());

        let keypair = wallet.decrypt(password.as_bytes())?;

        let mut txn = BlockchainTxnVarsV1 {
            version_predicate: self.version_predicate,
            master_key: keypair.pubkey_bin().into(),
            proof: vec![],
            key_proof: vec![],
            vars: self.set.iter().map(|v| v.var.clone()).collect(),
            nonce: self.nonce,
            unsets: self.unset.iter().map(|v| v.as_bytes().to_vec()).collect(),
            cancels: vec![],
            multi_key_proofs: vec![],
            multi_keys: vec![],
            multi_proofs: vec![],
        };

        let envelope = txn.sign(&keypair, Signer::Owner)?.in_envelope();
        let status = if self.commit {
            Some(client.submit_txn(&envelope)?)
        } else {
            None
        };

        print_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnVarsV1,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.add_row(row!["Set", "Value"]);
            for var in &txn.vars {
                let value = decode_var(&var)?;
                table.add_row(row![var.name, serde_json::to_string_pretty(&value)?]);
            }
            print_table(&table)?;

            // Handle unsets
            let mut table = Table::new();
            table.add_row(row!["Unset"]);
            for var in &txn.unsets {
                let name = String::from_utf8(var.to_vec())?;
                table.add_row(row![name]);
            }
            print_table(&table)?;

            ptable!(
                ["Key", "Value"],
                ["Nonce", txn.nonce],
                ["Hash", status_str(status)]
            );

            print_footer(status)
        }
        OutputFormat::Json => {
            let mut sets = Vec::with_capacity(txn.vars.len());
            for var in &txn.vars {
                sets.push(json!({
                    "name": var.name,
                    "value": decode_var(var)?
                }));
            }
            let mut unsets = Vec::with_capacity(txn.unsets.len());
            for var in &txn.unsets {
                unsets.push(json!(String::from_utf8(var.to_vec())?));
            }

            let table = json!({
                "sets": sets,
                "unsets": unsets,
                "hash": status_json(status),
                "txn": envelope.to_b64()?
            });

            print_json(&table)
        }
    }
}

fn print_vars(vars: &serde_json::Map<String, serde_json::Value>, format: OutputFormat) -> Result {
    match format {
        OutputFormat::Table => {
            let mut table = Table::new();
            table.add_row(row!["Name", "Value"]);
            for (name, value) in vars.iter() {
                table.add_row(row![name, serde_json::to_string_pretty(&value)?]);
            }
            print_table(&table)
        }
        OutputFormat::Json => print_json(&vars),
    }
}

fn decode_var(var: &BlockchainVarV1) -> Result<serde_json::Value> {
    match &var.r#type[..] {
        "integer" => {
            let value: i64 = String::from_utf8(var.value.to_vec())?.parse::<i64>()?;
            Ok(json!(value))
        }
        "float" => {
            let value: f64 = String::from_utf8(var.value.to_vec())?.parse::<f64>()?;
            Ok(json!(value))
        }
        "string" => {
            let value: String = String::from_utf8(var.value.to_vec())?;
            Ok(json!(value))
        }
        _ => Err(format!("Invalid variable {:?}", var).into()),
    }
}

#[derive(Debug)]
pub struct VarSet {
    var: BlockchainVarV1,
}

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
                r#type: "integer".to_string(),
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
        Ok(VarSet { var })
    }
}
