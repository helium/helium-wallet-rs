use crate::{
    cmd::{api_url, get_password, get_txn_fees, load_wallet, print_table, Opts},
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, TxnEnvelope, TxnFee, B58, B64},
};
use helium_api::{BlockchainTxn, BlockchainTxnTransferHotspotV1, Client, Hnt, Txn};
use std::io;
use structopt::StructOpt;

/// Transfer hotspot as buyer or seller.
#[derive(Debug, StructOpt)]
pub enum Transfer {
    Buy(Buy),
    Sell(Sell),
}

#[derive(Debug, StructOpt)]
pub struct Sell {
    /// Public address of gateway to be transferred
    gateway: String,
    /// The recipient of the gateway transfer
    buyer: String,
    /// Price in HNT to be paid by recipient of transfer
    price: Option<Hnt>,
    #[structopt(long)]
    /// Commit to sign and output the transaction in base64
    commit: bool,
}

#[derive(Debug, StructOpt)]
pub struct Buy {
    /// Base64 encoded transaction to sign. If no transaction if given
    /// stdin is read for the transaction. Note that the stdin feature
    /// only works if the wallet password is set in the
    /// HELIUM_WALLET_PASSWORD environment variable
    #[structopt(name = "TRANSACTION")]
    txn: Option<String>,
    #[structopt(long)]
    /// Commit to sign and submit the transaction
    commit: bool,
}

impl Buy {
    fn read_txn(&self) -> Result<String> {
        match &self.txn {
            Some(txn) => Ok(txn.to_string()),
            None => {
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
                Ok(buffer.trim().to_string())
            }
        }
    }
}

impl Transfer {
    pub fn run(self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;
        let client = Client::new_with_base_url(api_url());

        match self {
            Self::Sell(sell) => {
                let seller = wallet.address_as_vec();
                let buyer = PubKeyBin::from_b58(&sell.buyer)?;
                let buyer_account = client.get_account(&buyer.to_b58()?)?;
                let gateway = PubKeyBin::from_b58(&sell.gateway)?.to_vec();

                let mut txn = BlockchainTxnTransferHotspotV1 {
                    fee: 0,
                    seller,
                    gateway,
                    buyer: buyer.to_vec(),
                    seller_signature: vec![],
                    buyer_signature: vec![],
                    amount_to_seller: if let Some(price) = sell.price {
                        price.to_bones()
                    } else {
                        0
                    },
                    buyer_nonce: buyer_account.speculative_nonce + 1,
                };
                txn.fee = txn.txn_fee(&get_txn_fees(&client)?)?;
                print_txn_as_table(&txn)?;

                if sell.commit {
                    let password = get_password(false)?;
                    let keypair = wallet.decrypt(password.as_bytes())?;
                    txn.seller_signature = txn.sign(&keypair)?;
                    println!("{}", txn.in_envelope().to_b64()?);
                } else {
                    println!("Use --commit flag to sign transaction")
                }
            }

            Self::Buy(buy) => {
                let mut envelope = BlockchainTxn::from_b64(&buy.read_txn()?)?;

                match &mut envelope.txn {
                    Some(Txn::TransferHotspot(t)) => {
                        print_txn_as_table(&t)?;
                        if buy.commit {
                            let password = get_password(false)?;
                            let keypair = wallet.decrypt(password.as_bytes())?;
                            t.buyer_signature = t.sign(&keypair)?;

                            match client.submit_txn(&envelope) {
                                Ok(status) => {
                                    println!(
                                        "Successfully submitted txn with hash: {}",
                                        status.hash
                                    );
                                }
                                Err(e) => {
                                    println!("{}", e);
                                    println!("Submit failed. Please try again");
                                }
                            }
                        }
                    }
                    _ => return Err("Unsupported transaction for transfer_hotspot".into()),
                };
            }
        }

        Ok(())
    }
}

fn print_txn_as_table(txn: &BlockchainTxnTransferHotspotV1) -> Result {
    use prettytable::Table;
    let mut table = Table::new();
    let seller = PubKeyBin::from_vec(txn.seller.as_slice());
    let hotspot = PubKeyBin::from_vec(txn.gateway.as_slice());
    let buyer = PubKeyBin::from_vec(txn.buyer.as_slice());

    let sold_for = Hnt::from_bones(txn.amount_to_seller);
    table.add_row(row![
        "Seller",
        "Hotspot",
        "Sale Price [HNT]",
        "Buyer",
        "Buyer Nonce"
    ]);
    table.add_row(row![seller, hotspot, sold_for, buyer, txn.buyer_nonce]);
    print_table(&table)
}
