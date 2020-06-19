use super::TxnEnvelope;
use crate::result::Result;
use helium_api::{
    BlockchainTxnAddGatewayV1, BlockchainTxnAssertLocationV1, BlockchainTxnCreateHtlcV1,
    BlockchainTxnOuiV1, BlockchainTxnPaymentV1, BlockchainTxnPaymentV2, BlockchainTxnRedeemHtlcV1,
    BlockchainTxnSecurityExchangeV1, Message,
};

pub struct TxnFeeConfig {
    txn_fee_multiplier: u64,
    dc_payload_size: usize,
}

pub trait TxnFee {
    fn txn_fee(&self, config: &TxnFeeConfig) -> Result<u64>;
}

fn calculate_txn_fee(payload_size: usize, config: &TxnFeeConfig) -> u64 {
    if payload_size <= config.dc_payload_size {
        1
    } else {
        // integer div/ceil from: https://stackoverflow.com/a/2745086
        ((payload_size + config.dc_payload_size - 1) / config.dc_payload_size) as u64
    }
}

const TXN_FEE_SIGNATURE_SIZE: usize = 64;

macro_rules! impl_txn_fee {
    ($txn_type:ty, $( $sig:ident ),+ ) => {
        impl_txn_fee!($txn_type, {}, $( $sig ),+ );
    };
    // Detect the payer kind and construct a closure that will set the
    // payer signature right based on whether there is a payer or not.
    ($txn_type:ty, payer, $( $sig:ident ),+ ) => {
        let proc = | txn: &mut $txn_type | {
            if txn.payer.is_empty() {
                txn.payer_signature = vec![]
            } else {
                txn.payer_signature = vec![0, TXN_FEE_SIGNATURE_SUZE]
            }
        };
        impl_txn_fee!($txn_type, proc, $( $sig ),+ );
    };
    ($txn_type:ty, $proc: expr, $( $sig:ident ),+ ) => {
        impl TxnFee for $txn_type {
            fn txn_fee(&self, config: &TxnFeeConfig) -> Result<u64> {
                let mut txn: $txn_type = self.clone();
                txn.fee = 0;
                $(txn.$sig = vec![0; TXN_FEE_SIGNATURE_SIZE];)+
                $proc(&mut txn);
                let mut buf = vec![];
                txn.in_envelope().encode(&mut buf)?;
                Ok(calculate_txn_fee(buf.len(), config) * config.txn_fee_multiplier)
            }
        }
    }
}

impl_txn_fee!(BlockchainTxnPaymentV1, signature);
impl_txn_fee!(BlockchainTxnPaymentV2, signature);
impl_txn_fee!(BlockchainTxnCreateHtlcV1, signature);
impl_txn_fee!(BlockchainTxnRedeemHtlcV1, signature);
impl_txn_fee!(BlockchainTxnSecurityExchangeV1, signature);
impl_txn_fee!(
    BlockchainTxnAddGatewayV1,
    payer,
    owner_signature,
    gateway_signature
);
impl_txn_fee!(
    BlockchainTxnAssertLocationV1,
    payer,
    owner_signature,
    gateway_signature
);
impl_txn_fee!(BlockchainTxnOuiV1, payer, owner_signature);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keypair::Keypair;

    #[test]
    fn payment_v1_fee() {
        let payer = Keypair::gen_keypair();
        let payee = Keypair::gen_keypair();
        let fee_config = TxnFeeConfig {
            dc_payload_size: 24,
            txn_fee_multiplier: 5000,
        };
        let mut txn = BlockchainTxnPaymentV1 {
            payee: payee.pubkey_bin().into(),
            payer: payer.pubkey_bin().into(),
            amount: 0,
            nonce: 1,
            fee: 0,
            signature: vec![],
        };
        txn.amount = 10_000;
        txn.fee = txn.txn_fee(&fee_config).unwrap();
        assert_eq!(txn.fee, 30_000);

        txn.amount = 16_383;
        txn.fee = txn.txn_fee(&fee_config).unwrap();
        assert_eq!(txn.fee, 30_000);

        txn.amount = 16_384;
        txn.fee = txn.txn_fee(&fee_config).unwrap();
        assert_eq!(txn.fee, 35_000);
    }
}
