use crate::{
    cmd::{api_url, get_password, load_wallet, print_json, status_json, Opts},
    keypair::Keypair,
    result::Result,
    traits::{Sign, ToJson, B64},
};
use helium_api::{BlockchainTxn, Client, PendingTxnStatus, Txn};
use serde::{Deserialize, Serialize};
use std::{fs::File, path::PathBuf};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Commands multi signature transactions
pub enum Cmd {
    Inspect(Inspect),
    Sign(Prove),
    Combine(Combine),
}

#[derive(Debug, StructOpt)]
/// Inspect a given transaction artifact file
pub struct Inspect {
    #[structopt(name = "ARTIFACT FILE")]
    artifact: PathBuf,
}

#[derive(Debug, StructOpt)]
/// Sign a given transaction artifact file.
pub struct Prove {
    #[structopt(name = "ARTIFACT FILE")]
    artifact: PathBuf,

    /// Sign as a new key
    #[structopt(long = "key")]
    key: bool,
}

#[derive(Debug, StructOpt)]
/// Combine an artifact file with a number of proof files and optionally commit
/// to the Helium API.
pub struct Combine {
    #[structopt(name = "ARTIFACT FILE")]
    artifact: PathBuf,

    /// Proof file(s) to use
    #[structopt(long = "proof")]
    proofs: Vec<PathBuf>,

    /// Commit the combined transaction to the API
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Inspect(cmd) => cmd.run(opts),
            Cmd::Sign(cmd) => cmd.run(opts),
            Cmd::Combine(cmd) => cmd.run(opts),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct Artifact {
    txn: String,
}

enum ProofType {
    KeyProof,
    Proof,
}

#[derive(Serialize, Deserialize, Debug)]
struct Proofs {
    proofs: Vec<String>,
    key_proofs: Vec<String>,
}

impl Inspect {
    pub fn run(&self, _opts: Opts) -> Result {
        let txn = Artifact::load_txn(&self.artifact)?;
        print_txn(&txn, &None)
    }
}

impl Prove {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let txn = Artifact::load_txn(&self.artifact)?;
        let mut proofs = Proofs::new();
        let proof_type = if self.key {
            ProofType::KeyProof
        } else {
            ProofType::Proof
        };
        proofs.add_proof(&keypair, &txn, proof_type)?;
        print_json(&proofs)
    }
}

impl Combine {
    pub fn run(&self, _opts: Opts) -> Result {
        let mut envelope = Artifact::load_txn(&self.artifact)?;
        // Load proofs and key_proof maps from txn
        let mut combined_proofs = Proofs::from_txn(&envelope)?;
        for path in &self.proofs {
            let proofs = Proofs::load(path)?;
            combined_proofs.merge_proofs(&proofs)?;
        }
        combined_proofs.apply(&mut envelope)?;
        let status = if self.commit {
            let client = Client::new_with_base_url(api_url());
            Some(client.submit_txn(&envelope)?)
        } else {
            None
        };
        print_txn(&envelope, &status)
    }
}

fn print_txn(envelope: &BlockchainTxn, status: &Option<PendingTxnStatus>) -> Result {
    let mut json = match &envelope.txn {
        Some(Txn::Vars(t)) => t.to_json()?,
        _ => return Err("Unsupported transaction for multisig".into()),
    };
    json["hash"] = status_json(status);
    print_json(&json)
}

impl Proofs {
    fn new() -> Self {
        Proofs {
            proofs: Vec::default(),
            key_proofs: Vec::default(),
        }
    }

    fn load(path: &PathBuf) -> Result<Self> {
        let file = File::open(path)?;
        let proofs: Proofs = serde_json::from_reader(&file)?;
        Ok(proofs)
    }

    fn from_txn(envelope: &BlockchainTxn) -> Result<Self> {
        let mut proofs = Self::new();
        match &envelope.txn {
            Some(Txn::Vars(t)) => {
                for signature in &t.multi_key_proofs {
                    proofs.key_proofs.push(signature.to_b64()?);
                }
                let multi_proofs = t.multi_proofs.clone().into_iter();
                for signature in multi_proofs {
                    proofs.proofs.push(signature.to_b64()?);
                }
            }
            _ => return Err("Invalid transaction for proof".into()),
        }
        Ok(proofs)
    }

    fn apply(&self, envelope: &mut BlockchainTxn) -> Result {
        match &mut envelope.txn {
            Some(Txn::Vars(t)) => {
                t.multi_key_proofs = Vec::with_capacity(self.key_proofs.len());
                for signature in &self.key_proofs {
                    t.multi_key_proofs.push(Vec::<u8>::from_b64(&signature)?);
                }
                t.multi_proofs = Vec::with_capacity(self.proofs.len());
                for signature in &self.proofs {
                    t.multi_proofs.push(Vec::<u8>::from_b64(&signature)?);
                }
            }
            _ => return Err("Invalid transaction for proof".into()),
        };
        Ok(())
    }

    fn add_proof(
        &mut self,
        keypair: &Keypair,
        envelope: &BlockchainTxn,
        proof_type: ProofType,
    ) -> Result {
        match &envelope.txn {
            Some(Txn::Vars(t)) => {
                let signature = t.sign(&keypair)?.to_b64()?;
                match proof_type {
                    ProofType::KeyProof => self.key_proofs.push(signature),
                    ProofType::Proof => self.proofs.push(signature),
                };
                self.dedup();
            }
            _ => return Err("Invalid transaction for proof".into()),
        };
        Ok(())
    }

    fn merge_proofs(&mut self, other: &Proofs) -> Result {
        self.proofs.extend(other.proofs.clone());
        self.key_proofs.extend(other.key_proofs.clone());
        self.dedup();
        Ok(())
    }

    fn dedup(&mut self) {
        self.proofs.dedup_by(|a, b| a == b);
        self.key_proofs.dedup_by(|a, b| a == b)
    }
}

impl Artifact {
    fn load_txn(path: &PathBuf) -> Result<BlockchainTxn> {
        let file = File::open(path)?;
        let artifact: Artifact = serde_json::from_reader(&file)?;
        artifact.to_txn()
    }

    pub fn from_txn(txn: &BlockchainTxn) -> Result<Self> {
        Ok(Self { txn: txn.to_b64()? })
    }

    pub fn to_txn(&self) -> Result<BlockchainTxn> {
        Ok(BlockchainTxn::from_b64(&self.txn)?)
    }
}
