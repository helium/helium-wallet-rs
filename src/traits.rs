use crate::keypair::{Keypair, PubKeyBin, PublicKey, Verify, KEYTYPE_ED25519};
use crate::result::Result;
use helium_api::{
    BlockchainTxn, BlockchainTxnAddGatewayV1, BlockchainTxnAssertLocationV1,
    BlockchainTxnCreateHtlcV1, BlockchainTxnOuiV1, BlockchainTxnPaymentV1, BlockchainTxnPaymentV2,
    BlockchainTxnPriceOracleV1, BlockchainTxnRedeemHtlcV1, Message, Txn,
};
use io::{Read, Write};
use std::io;

pub trait ReadWrite {
    fn read(reader: &mut dyn Read) -> Result<Self>
    where
        Self: std::marker::Sized;
    fn write(&self, writer: &mut dyn Write) -> Result;
}

pub trait B58 {
    fn to_b58(&self) -> Result<String>;
    fn from_b58(str: &str) -> Result<Self>
    where
        Self: std::marker::Sized;
}

impl ReadWrite for PublicKey {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        let pubkey_bin: PubKeyBin = self.into();
        pubkey_bin.write(writer)
    }

    fn read(reader: &mut dyn Read) -> Result<PublicKey> {
        let pubkey_bin = PubKeyBin::read(reader)?;
        if pubkey_bin.0[0] != KEYTYPE_ED25519 {
            return Err(format!("Invalid key type {}", pubkey_bin.0[0]).into());
        }
        let pubkey: PublicKey = pubkey_bin.into();
        Ok(pubkey)
    }
}

impl B58 for PublicKey {
    fn to_b58(&self) -> Result<String> {
        let pubkey_bin: PubKeyBin = self.into();
        pubkey_bin.to_b58()
    }

    fn from_b58(b58: &str) -> Result<PublicKey> {
        let pubkey_bin = PubKeyBin::from_b58(b58)?;

        if pubkey_bin.0[0] != KEYTYPE_ED25519 {
            return Err(format!("Invalid key type {}", pubkey_bin.0[0]).into());
        }
        let pubkey: PublicKey = pubkey_bin.into();
        Ok(pubkey)
    }
}

impl ReadWrite for PubKeyBin {
    fn write(&self, writer: &mut dyn io::Write) -> Result {
        writer.write_all(&self.0)?;
        Ok(())
    }

    fn read(reader: &mut dyn Read) -> Result<Self> {
        let mut pubkey_bin = PubKeyBin::default();
        reader.read_exact(&mut pubkey_bin.0)?;
        Ok(pubkey_bin)
    }
}

impl B58 for PubKeyBin {
    fn to_b58(&self) -> Result<String> {
        // First 0 value is the "version" number defined for addresses
        // in libp2p
        let mut data = [0u8; 34];
        data[1..].copy_from_slice(&self.0);
        Ok(bs58::encode(data.as_ref()).with_check().into_string())
    }

    fn from_b58(b58: &str) -> Result<Self> {
        // First 0 value is the version byte
        let data = bs58::decode(b58).with_check(Some(0)).into_vec()?;
        let mut pubkey_bin = PubKeyBin::default();
        pubkey_bin.0.copy_from_slice(&data[1..]);
        Ok(pubkey_bin)
    }
}

pub trait B64 {
    fn to_b64(&self) -> Result<String>;
    fn from_b64(str: &str) -> Result<Self>
    where
        Self: std::marker::Sized;
}

impl B64 for BlockchainTxn {
    fn to_b64(&self) -> Result<String> {
        let mut buf = vec![];
        self.encode(&mut buf)?;
        Ok(base64::encode(&buf))
    }

    fn from_b64(b64: &str) -> Result<Self> {
        let decoded = base64::decode(b64)?;
        let envelope = BlockchainTxn::decode(&decoded[..])?;
        Ok(envelope)
    }
}

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

pub trait TxnPayer {
    fn payer(&self) -> Result<Option<PubKeyBin>>;
}

impl TxnPayer for BlockchainTxn {
    fn payer(&self) -> Result<Option<PubKeyBin>> {
        let maybe_payer = |v: &[u8]| {
            if v.is_empty() {
                None
            } else {
                Some(PubKeyBin::from_vec(v))
            }
        };
        match &self.txn {
            Some(Txn::AddGateway(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::AssertLocation(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::CreateHtlc(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::Payment(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::PaymentV2(t)) => Ok(maybe_payer(&t.payer)),
            Some(Txn::Oui(t)) => Ok(maybe_payer(&t.payer)),
            _ => Err("Unsupported transaction".into()),
        }
    }
}

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
