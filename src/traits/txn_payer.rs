use crate::{
    keypair::PublicKey,
    result::{anyhow, Result},
};
use helium_proto::*;

pub trait TxnPayer {
    fn payer(&self) -> Result<Option<PublicKey>>;
}

impl TxnPayer for BlockchainTxn {
    fn payer(&self) -> Result<Option<PublicKey>> {
        let maybe_payer = |v: &[u8]| {
            if v.is_empty() {
                None
            } else {
                Some(PublicKey::from_bytes(v).ok()?)
            }
        };
        match &self.txn {
            Some(Txn::AddGateway(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::AssertLocation(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::CreateHtlc(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::Payment(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::PaymentV2(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::Oui(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::TokenBurn(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::TransferHotspot(t)) => Ok(maybe_payer(&t.buyer)),
            _ => Err(anyhow!("Unsupported transaction")),
        }
    }
}
