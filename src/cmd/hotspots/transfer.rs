use crate::{
    cmd::{
        api_url, get_password, get_txn_fees, load_wallet, print_json, status_json, Opts,
        OutputFormat,
    },
    keypair::PubKeyBin,
    result::Result,
    traits::{Sign, ToJson, TxnEnvelope, TxnFee, B58, B64},
};
use helium_api::{
    BlockchainTxn, BlockchainTxnTransferHotspotV1, Client, Hnt, PendingTxnStatus, Txn,
};
use std::io;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Transfer hotspot as buyer or seller.
pub enum Transfer {
    /// Create and sign transaction to sell a hotspot, outputting it as base64 for counter-party
    Sell(Sell),
    /// Ingest a transaction to buy a hotspot from base64.
    /// Signs and submits the transaction to the API
    Buy(Buy),
}

#[derive(Debug, StructOpt)]
pub struct Sell {
    /// Public address of gateway to be transferred
    gateway: String,
    /// The recipient of the gateway transfer
    buyer: String,
    /// Price in HNT to be paid by recipient of transfer
    price: Option<Hnt>,
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
                let seller = wallet.pubkey_bin.to_vec();
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
                eprintln!("{:#?}", txn.to_json()?);

                let password = get_password(false)?;
                let keypair = wallet.decrypt(password.as_bytes())?;
                txn.seller_signature = txn.sign(&keypair)?;
                eprintln!("{}", txn.in_envelope().to_b64()?);
                Ok(())
            }

            Self::Buy(buy) => {
                let mut envelope = BlockchainTxn::from_b64(&buy.read_txn()?)?;

                match &mut envelope.txn {
                    Some(Txn::TransferHotspot(t)) => {
                        // verify that nonce is still valid.
                        let nonce = t.buyer_nonce;
                        let buyer_account =
                            client.get_account(&PubKeyBin::from_vec(&t.buyer).to_b58()?)?;
                        let expected_nonce = buyer_account.speculative_nonce + 1;

                        if buyer_account.speculative_nonce + 1 != nonce {
                            eprintln!(
                                "Buyer_nonce in transaction is {} while expected nonce is {}",
                                nonce, expected_nonce
                            );
                            return Err("Hotspot transfer nonce no longer valid".into());
                        }

                        let password = get_password(false)?;
                        let keypair = wallet.decrypt(password.as_bytes())?;
                        t.buyer_signature = t.sign(&keypair)?;
                        let status = if buy.commit {
                            Some(client.submit_txn(&envelope)?)
                        } else {
                            None
                        };
                        print_txn(&envelope, &status, opts.format)
                    }
                    _ => Err("Unsupported transaction for transfer_hotspot".into()),
                }
            }
        }
    }
}

fn print_txn(
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let encoded = envelope.to_b64()?;
    match format {
        OutputFormat::Table => Err("Table format not supported for transaction output".into()),
        OutputFormat::Json => {
            let table = json!({
                "txn": encoded,
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}
