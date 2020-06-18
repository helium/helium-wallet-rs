use helium_api::{
    BlockchainTxn, BlockchainTxnAddGatewayV1, BlockchainTxnAssertLocationV1,
    BlockchainTxnCreateHtlcV1, BlockchainTxnOuiV1, BlockchainTxnPaymentV1, BlockchainTxnPaymentV2,
    BlockchainTxnPriceOracleV1, BlockchainTxnRedeemHtlcV1, BlockchainTxnSecurityExchangeV1, Txn,
};

pub trait TxnEnvelope {
    fn in_envelope(&self) -> BlockchainTxn;
}

impl TxnEnvelope for BlockchainTxnPriceOracleV1 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::PriceOracleSubmission(self.clone())),
        }
    }
}

impl TxnEnvelope for BlockchainTxnOuiV1 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::Oui(self.clone())),
        }
    }
}

impl TxnEnvelope for BlockchainTxnCreateHtlcV1 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::CreateHtlc(self.clone())),
        }
    }
}

impl TxnEnvelope for BlockchainTxnRedeemHtlcV1 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::RedeemHtlc(self.clone())),
        }
    }
}

impl TxnEnvelope for BlockchainTxnPaymentV1 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::Payment(self.clone())),
        }
    }
}

impl TxnEnvelope for BlockchainTxnPaymentV2 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::PaymentV2(self.clone())),
        }
    }
}

impl TxnEnvelope for BlockchainTxnSecurityExchangeV1 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::SecurityExchange(self.clone())),
        }
    }
}

impl TxnEnvelope for BlockchainTxnAddGatewayV1 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::AddGateway(self.clone())),
        }
    }
}

impl TxnEnvelope for BlockchainTxnAssertLocationV1 {
    fn in_envelope(&self) -> BlockchainTxn {
        BlockchainTxn {
            txn: Some(Txn::AssertLocation(self.clone())),
        }
    }
}
