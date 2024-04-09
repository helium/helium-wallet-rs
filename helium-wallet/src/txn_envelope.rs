use crate::result::{anyhow, Result};
use helium_proto::*;

pub trait TxnEnvelope {
    fn in_envelope(&self) -> BlockchainTxn;
    fn from_envelope(envelope: &BlockchainTxn) -> Result<Self>
    where
        Self: Sized;
}

macro_rules! impl_txn_envelope {
    ($txn_type: ty, $kind: ident) => {
        impl TxnEnvelope for $txn_type {
            fn in_envelope(&self) -> BlockchainTxn {
                BlockchainTxn {
                    txn: Some(Txn::$kind(self.clone())),
                }
            }

            fn from_envelope(envelope: &BlockchainTxn) -> Result<Self> {
                match &envelope.txn {
                    Some(Txn::$kind(txn)) => Ok(txn.clone()),
                    _ => Err(anyhow!("unsupported transaction")),
                }
            }
        }
    };
}

impl_txn_envelope!(BlockchainTxnAddGatewayV1, AddGateway);
