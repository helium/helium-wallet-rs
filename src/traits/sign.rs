use crate::keypair::{Keypair, PublicKey, Verify};
use crate::result::Result;
use helium_api::{
    BlockchainTxn, BlockchainTxnAddGatewayV1, BlockchainTxnAssertLocationV1,
    BlockchainTxnCreateHtlcV1, BlockchainTxnOuiV1, BlockchainTxnPaymentV1, BlockchainTxnPaymentV2,
    BlockchainTxnPriceOracleV1, BlockchainTxnRedeemHtlcV1, BlockchainTxnSecurityExchangeV1,
    Message, Txn,
};

#[derive(PartialEq)]
pub enum Signer {
    Owner,
    Payer,
    Gateway,
}

pub trait Sign: Message + std::clone::Clone {
    fn sign(&mut self, keypair: &Keypair, signer: Signer) -> Result<&mut Self>
    where
        Self: std::marker::Sized,
    {
        let mut buf = vec![];
        let mut signed = self.clone();
        signed.clear_signatures();
        signed.encode(&mut buf)?;
        self.set_signature(signer, keypair.sign(&buf))?;
        Ok(self)
    }

    fn verify(&self, pubkey: &PublicKey, signer: Signer) -> Result {
        let mut buf = vec![];
        let mut signed = self.clone();
        signed.clear_signatures();
        signed.encode(&mut buf)?;
        pubkey.verify(&buf, &self.get_signature(signer)?)
    }

    fn clear_signatures(&mut self);
    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result;
    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>>;
}

impl Sign for BlockchainTxn {
    fn sign(&mut self, keypair: &Keypair, signer: Signer) -> Result<&mut Self> {
        match &mut self.txn {
            Some(Txn::PaymentV2(t)) => {
                t.sign(keypair, signer)?;
                Ok(self)
            }
            Some(Txn::CreateHtlc(t)) => {
                t.sign(keypair, signer)?;
                Ok(self)
            }
            Some(Txn::RedeemHtlc(t)) => {
                t.sign(keypair, signer)?;
                Ok(self)
            }
            Some(Txn::AddGateway(t)) => {
                t.sign(keypair, signer)?;
                Ok(self)
            }
            Some(Txn::AssertLocation(t)) => {
                t.sign(keypair, signer)?;
                Ok(self)
            }
            Some(Txn::Oui(t)) => {
                t.sign(keypair, signer)?;
                Ok(self)
            }
            Some(Txn::PriceOracleSubmission(t)) => {
                t.sign(keypair, signer)?;
                Ok(self)
            }
            Some(Txn::SecurityExchange(t)) => {
                t.sign(keypair, signer)?;
                Ok(self)
            }
            _ => Err("Unsupported transaction for signing".into()),
        }
    }

    fn clear_signatures(&mut self) {}
    fn set_signature(&mut self, _signer: Signer, _signature: Vec<u8>) -> Result {
        Ok(())
    }
    fn get_signature(&self, _signer: Signer) -> Result<Vec<u8>> {
        Err("Invalid transaction".into())
    }
}

impl Sign for BlockchainTxnPriceOracleV1 {
    fn clear_signatures(&mut self) {
        self.signature = vec![]
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Owner => self.signature = signature,
            _ => return Err("Invalid signer".into()),
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Owner => Ok(self.signature.clone()),
            _ => Err("Invalid signer".into()),
        }
    }
}

impl Sign for BlockchainTxnPaymentV1 {
    fn clear_signatures(&mut self) {
        self.signature = vec![]
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Payer => self.signature = signature,
            _ => return Err("Invalid signer".into()),
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Payer => Ok(self.signature.clone()),
            _ => Err("Invalid signer".into()),
        }
    }
}

impl Sign for BlockchainTxnPaymentV2 {
    fn clear_signatures(&mut self) {
        self.signature = vec![]
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Payer => self.signature = signature,
            _ => return Err("Invalid signer".into()),
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Payer => Ok(self.signature.clone()),
            _ => Err("Invalid signer".into()),
        }
    }
}

impl Sign for BlockchainTxnCreateHtlcV1 {
    fn clear_signatures(&mut self) {
        self.signature = vec![]
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Payer => self.signature = signature,
            _ => return Err("Invalid signer".into()),
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Payer => Ok(self.signature.clone()),
            _ => Err("Invalid signer".into()),
        }
    }
}

impl Sign for BlockchainTxnRedeemHtlcV1 {
    fn clear_signatures(&mut self) {
        self.signature = vec![]
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Owner => self.signature = signature,
            _ => return Err("Invalid signer".into()),
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Owner => Ok(self.signature.clone()),
            _ => Err("Invalid signer".into()),
        }
    }
}

impl Sign for BlockchainTxnAddGatewayV1 {
    fn clear_signatures(&mut self) {
        self.owner_signature = vec![];
        self.payer_signature = vec![];
        self.gateway_signature = vec![];
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Owner => self.owner_signature = signature,
            Signer::Payer => self.payer_signature = signature,
            Signer::Gateway => self.gateway_signature = signature,
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Owner => Ok(self.owner_signature.clone()),
            Signer::Payer => Ok(self.payer_signature.clone()),
            Signer::Gateway => Ok(self.gateway_signature.clone()),
        }
    }
}

impl Sign for BlockchainTxnAssertLocationV1 {
    fn clear_signatures(&mut self) {
        self.owner_signature = vec![];
        self.payer_signature = vec![];
        self.gateway_signature = vec![];
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Owner => self.owner_signature = signature,
            Signer::Payer => self.payer_signature = signature,
            Signer::Gateway => self.gateway_signature = signature,
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Owner => Ok(self.owner_signature.clone()),
            Signer::Payer => Ok(self.payer_signature.clone()),
            Signer::Gateway => Ok(self.gateway_signature.clone()),
        }
    }
}

impl Sign for BlockchainTxnOuiV1 {
    fn clear_signatures(&mut self) {
        self.owner_signature = vec![];
        self.payer_signature = vec![];
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Owner => self.owner_signature = signature,
            Signer::Payer => self.payer_signature = signature,
            _ => return Err("Unsupported signer".into()),
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Owner => Ok(self.owner_signature.clone()),
            Signer::Payer => Ok(self.payer_signature.clone()),
            _ => Err("Unsupported signer".into()),
        }
    }
}

impl Sign for BlockchainTxnSecurityExchangeV1 {
    fn clear_signatures(&mut self) {
        self.signature = vec![]
    }

    fn set_signature(&mut self, signer: Signer, signature: Vec<u8>) -> Result {
        match signer {
            Signer::Payer => self.signature = signature,
            _ => return Err("Invalid signer".into()),
        };
        Ok(())
    }

    fn get_signature(&self, signer: Signer) -> Result<Vec<u8>> {
        match signer {
            Signer::Payer => Ok(self.signature.clone()),
            _ => Err("Invalid signer".into()),
        }
    }
}
