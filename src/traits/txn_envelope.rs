use helium_api::{
    BlockchainTxn, BlockchainTxnAddGatewayV1, BlockchainTxnAssertLocationV1,
    BlockchainTxnCreateHtlcV1, BlockchainTxnOuiV1, BlockchainTxnPaymentV1, BlockchainTxnPaymentV2,
    BlockchainTxnPriceOracleV1, BlockchainTxnRedeemHtlcV1, BlockchainTxnSecurityExchangeV1,
    BlockchainTxnVarsV1, Txn,
};

pub trait TxnEnvelope {
    fn in_envelope(&self) -> BlockchainTxn;
}

macro_rules! impl_txn_envelope {
    ($txn_type: ty, $kind: ident) => {
        impl TxnEnvelope for $txn_type {
            fn in_envelope(&self) -> BlockchainTxn {
                BlockchainTxn {
                    txn: Some(Txn::$kind(self.clone())),
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
impl_txn_envelope!(BlockchainTxnAddGatewayV1, AddGateway);
impl_txn_envelope!(BlockchainTxnAssertLocationV1, AssertLocation);
impl_txn_envelope!(BlockchainTxnVarsV1, Vars);
