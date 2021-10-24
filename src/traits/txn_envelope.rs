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

impl_txn_envelope!(BlockchainTxnPriceOracleV1, PriceOracleSubmission);
impl_txn_envelope!(BlockchainTxnOuiV1, Oui);
impl_txn_envelope!(BlockchainTxnCreateHtlcV1, CreateHtlc);
impl_txn_envelope!(BlockchainTxnRedeemHtlcV1, RedeemHtlc);
impl_txn_envelope!(BlockchainTxnPaymentV1, Payment);
impl_txn_envelope!(BlockchainTxnPaymentV2, PaymentV2);
impl_txn_envelope!(BlockchainTxnSecurityExchangeV1, SecurityExchange);
impl_txn_envelope!(BlockchainTxnTokenBurnV1, TokenBurn);
impl_txn_envelope!(BlockchainTxnAddGatewayV1, AddGateway);
impl_txn_envelope!(BlockchainTxnAssertLocationV1, AssertLocation);
impl_txn_envelope!(BlockchainTxnAssertLocationV2, AssertLocationV2);
impl_txn_envelope!(BlockchainTxnVarsV1, Vars);
impl_txn_envelope!(BlockchainTxnTransferHotspotV1, TransferHotspot);
impl_txn_envelope!(BlockchainTxnTransferHotspotV2, TransferHotspotV2);
impl_txn_envelope!(BlockchainTxnStakeValidatorV1, StakeValidator);
impl_txn_envelope!(BlockchainTxnUnstakeValidatorV1, UnstakeValidator);
impl_txn_envelope!(BlockchainTxnTransferValidatorStakeV1, TransferValStake);
impl_txn_envelope!(BlockchainTxnRoutingV1, Routing);
