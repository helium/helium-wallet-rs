use crate::{
    cmd::*,
    keypair::{KeyTag, KeyType, Keypair, NETTYPE_MAIN_STR},
    result::Result,
    traits::ReadWrite,
};
use angry_purple_tiger::AnimalName;
use std::path::PathBuf;

#[derive(Debug, StructOpt)]
/// Create a keypair ready for a validator to use.
pub struct Cmd {
    /// Output file to store the key in
    #[structopt(short, long, default_value = "swarm_key")]
    output: PathBuf,

    #[structopt(long)]
    /// Overwrite an existing file
    force: bool,

    #[structopt(long, default_value = NETTYPE_MAIN_STR)]
    /// The network to generate the keypair on (testnet/mainnet)
    network: Network,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let tag = KeyTag {
            network: self.network,
            key_type: KeyType::EccCompact,
        };
        let keypair = Keypair::generate(tag);
        let mut writer = open_output_file(&self.output, !self.force)?;
        keypair.write(&mut writer)?;
        print_keypair(&keypair, opts.format)
    }
}

fn print_keypair(keypair: &Keypair, format: OutputFormat) -> Result {
    let address = keypair.public_key().to_string();
    let name = address.parse::<AnimalName>()?.to_string();
    match format {
        OutputFormat::Table => {
            ptable!(["Key", "Value"], ["Address", address], ["Name", name]);
            Ok(())
        }
        OutputFormat::Json => {
            let table = json!({
                "address": address,
                "name": name,
            });
            print_json(&table)
        }
    }
}
