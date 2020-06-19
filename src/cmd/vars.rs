use crate::{
    cmd::{api_url, get_password, load_wallet, Opts, OutputFormat},
    result::Result,
};
use helium_api::{BlockchainTxnVarsV1, BlockchainVarV1, Client};
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
/// Lists current chain variables
pub struct Create {
    #[structopt(long, default_value = "0")]
    version_predicate: u32,

    #[structopt(long, number_of_values(1))]
    set: Vec<VarSet>,

    #[structopt(long, number_of_values(1))]
    unset: Vec<String>,

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
        let _client = Client::new_with_base_url(api_url());

        let keypair = wallet.decrypt(password.as_bytes())?;

        let txn = BlockchainTxnVarsV1 {
            version_predicate: self.version_predicate,
            master_key: keypair.pubkey_bin().into(),
            proof: vec![],
            key_proof: vec![],
            vars: self.set.iter().map(|v| v.var.clone()).collect(),
            nonce: self.nonce,
            unsets: self.unset.iter().map(|v| v.as_bytes().to_vec()).collect(),
            cancels: vec![],
        };
        println!("{:?}", txn);

        Ok(())
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
            table.printstd();
            Ok(())
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&vars)?);
            Ok(())
        }
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
            serde_json::Value::Number(n) if n.is_u64() => BlockchainVarV1 {
                name: name,
                r#type: "integer".to_string(),
                value: n.to_string().as_bytes().to_vec(),
            },
            serde_json::Value::Number(n) if n.is_f64() => BlockchainVarV1 {
                name: name,
                r#type: "float".to_string(),
                value: n.to_string().as_bytes().to_vec(),
            },
            serde_json::Value::String(s) => BlockchainVarV1 {
                name: name,
                r#type: "string".to_string(),
                value: s.as_bytes().to_vec(),
            },
            _ => return Err(format!("Invalid variable value {}", value.to_string()).into()),
        };
        Ok(VarSet { var: var })
    }
}
