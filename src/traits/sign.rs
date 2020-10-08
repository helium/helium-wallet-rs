use crate::keypair::{Keypair, PublicKey, Verify};
use crate::result::Result;
use helium_api::{
    BlockchainTxnAddGatewayV1, BlockchainTxnAssertLocationV1, BlockchainTxnCreateHtlcV1,
    BlockchainTxnOuiV1, BlockchainTxnPaymentV1, BlockchainTxnPaymentV2, BlockchainTxnPriceOracleV1,
    BlockchainTxnRedeemHtlcV1, BlockchainTxnSecurityExchangeV1, BlockchainTxnTokenBurnV1,
    BlockchainTxnVarsV1, Message,
};

pub trait Sign: Message + std::clone::Clone {
    fn sign(&self, keypair: &Keypair) -> Result<Vec<u8>>
    where
        Self: std::marker::Sized;
    fn verify(&self, pubkey: &PublicKey, signature: &[u8]) -> Result;
}

macro_rules! impl_sign {
    ($txn_type:ty, $( $sig: ident ),+ ) => {
        impl Sign for $txn_type {
            fn sign(&self, keypair: &Keypair) -> Result<Vec<u8>> {
                let mut buf = vec![];
                let mut txn = self.clone();
                $(txn.$sig = vec![];)+
                txn.encode(& mut buf)?;
                Ok(keypair.sign(&buf))
            }

            fn verify(&self, pubkey: &PublicKey, signature: &[u8]) -> Result {
                let mut buf = vec![];
                let mut txn = self.clone();
                $(txn.$sig = vec![];)+
                txn.encode(& mut buf)?;
                pubkey.verify(&buf, &signature)
            }
        }
    }
}

impl_sign!(BlockchainTxnPriceOracleV1, signature);
impl_sign!(BlockchainTxnPaymentV1, signature);
impl_sign!(BlockchainTxnPaymentV2, signature);
impl_sign!(BlockchainTxnCreateHtlcV1, signature);
impl_sign!(BlockchainTxnRedeemHtlcV1, signature);
impl_sign!(
    BlockchainTxnAddGatewayV1,
    owner_signature,
    payer_signature,
    gateway_signature
);
impl_sign!(
    BlockchainTxnAssertLocationV1,
    owner_signature,
    payer_signature,
    gateway_signature
);
impl_sign!(BlockchainTxnOuiV1, owner_signature, payer_signature);
impl_sign!(BlockchainTxnSecurityExchangeV1, signature);
impl_sign!(BlockchainTxnTokenBurnV1, signature);
impl_sign!(
    BlockchainTxnVarsV1,
    proof,
    key_proof,
    multi_proofs,
    multi_key_proofs
);
