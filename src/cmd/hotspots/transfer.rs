use crate::{
    cmd::*,
    keypair::PublicKey,
    result::{anyhow, bail, Result},
    traits::{TxnEnvelope, TxnFee, TxnSign, B64},
};
use helium_api::{
    BlockchainTxn, BlockchainTxnTransferHotspotV1, Client, Hnt, PendingTxnStatus, Txn,
};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
/// Transfer hotspot as buyer or seller.
pub enum Cmd {
    /// Create and sign transaction to sell a hotspot, outputting it as base64 for counter-party
    Sell(Sell),
    /// Ingest a transaction to buy a hotspot from base64.
    /// Signs and submits the transaction to the API
    Buy(Buy),
}

#[derive(Debug, StructOpt)]
pub struct Sell {
    /// Public address of gateway to be transferred
    gateway: PublicKey,
    /// The recipient of the gateway transfer
    buyer: PublicKey,
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
    txn: Option<Transaction>,
    #[structopt(long)]
    /// Commit to sign and submit the transaction
    commit: bool,
}

impl Cmd {
    pub fn run(self, opts: Opts) -> Result {
        let wallet = load_wallet(opts.files)?;
        let client = Client::new_with_base_url(api_url(wallet.public_key.network));

        match self {
            Self::Sell(sell) => {
                let buyer_account = client.get_account(&sell.buyer.to_string())?;

                let mut txn = BlockchainTxnTransferHotspotV1 {
                    fee: 0,
                    seller: wallet.public_key.to_vec(),
                    gateway: sell.gateway.into(),
                    buyer: sell.buyer.into(),
                    seller_signature: vec![],
                    buyer_signature: vec![],
                    amount_to_seller: sell.price.unwrap_or_else(|| Hnt::from_bones(0)).to_bones(),
                    buyer_nonce: buyer_account.speculative_nonce + 1,
                };
                txn.fee = txn.txn_fee(&get_txn_fees(&client)?)?;
                let password = get_password(false)?;
                let keypair = wallet.decrypt(password.as_bytes())?;
                txn.seller_signature = txn.sign(&keypair)?;
                println!("{}", txn.in_envelope().to_b64()?);
                Ok(())
            }

            Self::Buy(buy) => {
                let mut envelope = read_txn(&buy.txn)?;

                match &mut envelope.txn {
                    Some(Txn::TransferHotspot(t)) => {
                        // verify that nonce is still valid.
                        let nonce = t.buyer_nonce;
                        let buyer_account =
                            client.get_account(&PublicKey::from_bytes(&t.buyer)?.to_string())?;
                        let expected_nonce = buyer_account.speculative_nonce + 1;

                        if buyer_account.speculative_nonce + 1 != nonce {
                            eprintln!(
                                "Buyer_nonce in transaction is {} while expected nonce is {}",
                                nonce, expected_nonce
                            );
                            bail!("Hotspot transfer nonce no longer valid");
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
                    _ => Err(anyhow!("Unsupported transaction for transfer_hotspot")),
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
        OutputFormat::Table => Err(anyhow!("Table format not supported for transaction output")),
        OutputFormat::Json => {
            let table = json!({
                "txn": encoded,
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}
