use crate::{
    cmd::{api_url, get_password, load_wallet, Opts},
    keypair::{Keypair, PubKeyBin},
    result::Result,
    traits::{Sign, B58},
};
use helium_api::{Client, Hnt, PendingTxnStatus};
use helium_proto::{BlockchainTxnCreateHtlcV1, BlockchainTxnRedeemHtlcV1, Txn};
use prettytable::Table;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Create or Redeem from an HTLC address
pub enum Cmd {
    Create(Create),
    Redeem(Redeem),
}

#[derive(Debug, StructOpt)]
/// Creates a new HTLC address with a specified hashlock and timelock (in block height), and transfers a value of tokens to it.
/// The transaction is not submitted to the system unless the '--commit' option is given.
pub struct Create {
    /// The address of the intended payee for this HTLC
    payee: String,

    /// Number of hnt to send
    #[structopt(long)]
    hnt: Hnt,

    /// A hex encoded SHA256 digest of a secret value (called a preimage) that locks this contract
    #[structopt(short = "h", long = "hashlock")]
    hashlock: String,

    /// A specific blockheight after which the payer (you) can redeem their tokens
    #[structopt(short = "t", long = "timelock")]
    timelock: u64,

    /// Commit the payment to the API
    #[structopt(long)]
    commit: bool,

    /// Only output the submitted transaction hash.
    #[structopt(long)]
    hash: bool,
}

#[derive(Debug, StructOpt)]
/// Redeem the balance from an HTLC address with the specified preimage for the hashlock
pub struct Redeem {
    /// Address of the HTLC contract to redeem from
    address: String,

    /// The preimage used to create the hashlock for this contract address
    #[structopt(short = "p", long = "preimage")]
    preimage: String,

    /// Only output the submitted transaction hash.
    #[structopt(long)]
    hash: bool,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        match self {
            Cmd::Create(cmd) => cmd.run(opts),
            Cmd::Redeem(cmd) => cmd.run(opts),
        }
    }
}

impl Create {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let client = Client::new_with_base_url(api_url());

        let keypair = wallet.to_keypair(password.as_bytes())?;
        let account = client.get_account(&keypair.public.to_b58()?)?;
        let address = Keypair::gen_keypair().pubkey_bin();

        let mut txn = BlockchainTxnCreateHtlcV1 {
            amount: self.hnt.to_bones(),
            fee: 0,
            payee: PubKeyBin::from_b58(self.payee.clone())?.to_vec(),
            payer: keypair.pubkey_bin().to_vec(),
            address: address.to_vec(),
            hashlock: hex::decode(self.hashlock.clone()).unwrap(),
            timelock: self.timelock,
            nonce: account.speculative_nonce + 1,
            signature: Vec::new(),
        };
        txn.sign(&keypair)?;
        let wrapped_txn = Txn::CreateHtlc(txn.clone());

        let status = if self.commit {
            Some(client.submit_txn(wrapped_txn)?)
        } else {
            None
        };

        if self.hash {
            println!("{}", status.map_or("none".to_string(), |s| s.hash));
        } else {
            print_create_txn(&txn, &status);
        }

        Ok(())
    }
}

fn print_create_txn(txn: &BlockchainTxnCreateHtlcV1, status: &Option<PendingTxnStatus>) {
    let mut table = Table::new();
    table.add_row(row!["Address", "Payee", "Amount", "Hashlock", "Timelock"]);
    table.add_row(row![
        PubKeyBin::from_vec(&txn.address).to_b58().unwrap(),
        PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
        txn.amount,
        hex::encode(&txn.hashlock),
        txn.timelock
    ]);
    table.printstd();

    if status.is_some() {
        ptable!(
            ["Nonce", "Hash"],
            [txn.nonce, status.as_ref().map_or("none", |s| &s.hash)]
        );
    }
}

impl Redeem {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.to_keypair(password.as_bytes())?;
        let client = Client::new_with_base_url(api_url());

        let mut txn = BlockchainTxnRedeemHtlcV1 {
            fee: 0,
            payee: keypair.pubkey_bin().to_vec(),
            address: PubKeyBin::from_b58(self.address.clone())?.to_vec(),
            preimage: self.preimage.clone().into_bytes(),
            signature: Vec::new(),
        };
        txn.sign(&keypair)?;
        let wrapped_txn = Txn::RedeemHtlc(txn.clone());

        let status = client.submit_txn(wrapped_txn)?;

        if self.hash {
            println!("{}", status.hash);
        } else {
            print_redeem_txn(&txn, &status);
        }

        Ok(())
    }
}

fn print_redeem_txn(txn: &BlockchainTxnRedeemHtlcV1, status: &PendingTxnStatus) {
    let mut table = Table::new();
    table.add_row(row!["Payee", "Address", "Preimage", "Hash"]);
    table.add_row(row![
        PubKeyBin::from_vec(&txn.payee).to_b58().unwrap(),
        PubKeyBin::from_vec(&txn.address).to_b58().unwrap(),
        std::str::from_utf8(&txn.preimage).unwrap(),
        status.hash
    ]);
    table.printstd();
}
