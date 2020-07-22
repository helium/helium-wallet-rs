use crate::keypair::{Keypair, PublicKey, Verify};
use crate::result::Result;
use helium_api::{
    BlockchainTxnAddGatewayV1, BlockchainTxnAssertLocationV1, BlockchainTxnCreateHtlcV1,
    BlockchainTxnOuiV1, BlockchainTxnPaymentV1, BlockchainTxnPaymentV2, BlockchainTxnPriceOracleV1,
    BlockchainTxnRedeemHtlcV1, BlockchainTxnSecurityExchangeV1, BlockchainTxnTokenBurnV1,
    BlockchainTxnVarsV1, Message,
};

#[derive(PartialEq)]
pub enum Signer {
    Owner,
    Payer,
    Gateway,
    // Adds an unknown signer to allow the macro get/set_signature
    // match arms to work
    Unknown,
}

pub trait Sign: Message + std::clone::Clone {
    fn sign(&mut self, keypair: &Keypair, signer: Signer) -> Result<&mut Self>
    where
        Self: std::marker::Sized;
    fn verify(&self, pubkey: &PublicKey, signer: Signer) -> Result;

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result;
    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>>;
}

macro_rules! impl_sign {
    ($txn_type:ty, $( ($signer: ident, $sig: ident) ),+ ) => {
        impl Sign for $txn_type {
            fn sign(&mut self, keypair: &Keypair, signer: Signer) -> Result<&mut Self> {
                let mut buf = vec![];
                let mut txn = self.clone();
                $(txn.$sig = vec![];)+
                txn.encode(& mut buf)?;
                self.set_signature(signer, keypair.sign(&buf))?;
                Ok(self)
            }

            fn verify(&self, pubkey: &PublicKey, signer: Signer) -> Result {
                let mut buf = vec![];
                let mut txn = self.clone();
                $(txn.$sig = vec![];)+
                txn.encode(& mut buf)?;
                pubkey.verify(&buf, &self.get_signature(signer)?)
            }

            fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
                match signer {
                    $( Signer::$signer => self.$sig = signature,)+
                    _ => return Err("Invalid signer".into()),
                };
                Ok(())
            }

            fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
                match signer {
                    $( Signer::$signer => Ok(self.$sig.clone()), )+
                    _ => Err("Invalid signer".into()),
                }
            }
        }
    }
}

impl_sign!(BlockchainTxnPriceOracleV1, (Owner, signature));
impl_sign!(BlockchainTxnPaymentV1, (Payer, signature));
impl_sign!(BlockchainTxnPaymentV2, (Payer, signature));
impl_sign!(BlockchainTxnCreateHtlcV1, (Payer, signature));
impl_sign!(BlockchainTxnRedeemHtlcV1, (Owner, signature));
impl_sign!(
    BlockchainTxnAddGatewayV1,
    (Owner, owner_signature),
    (Payer, payer_signature),
    (Gateway, gateway_signature)
);
impl_sign!(
    BlockchainTxnAssertLocationV1,
    (Owner, owner_signature),
    (Payer, payer_signature),
    (Gateway, gateway_signature)
);
impl_sign!(
    BlockchainTxnOuiV1,
    (Owner, owner_signature),
    (Payer, payer_signature)
);
impl_sign!(BlockchainTxnSecurityExchangeV1, (Payer, signature));
impl_sign!(BlockchainTxnTokenBurnV1, (Payer, signature));
impl_sign!(BlockchainTxnVarsV1, (Owner, proof));
