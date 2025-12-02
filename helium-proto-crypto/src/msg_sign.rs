use helium_crypto::{Keypair, Sign};
use msg_signature::MsgHasSignature;

#[derive(thiserror::Error, Debug)]
#[error("error signing message: {0}")]
pub struct MsgSignError(#[from] pub helium_crypto::Error);

pub trait MsgSign {
    fn sign(&mut self, signing_key: &Keypair) -> Result<(), MsgSignError>;
}

impl<T> MsgSign for T
where
    T: MsgHasSignature + prost::Message,
{
    fn sign(&mut self, signing_key: &Keypair) -> Result<(), MsgSignError> {
        self.clear_signature();
        let signature = signing_key.sign(&self.encode_to_vec())?;
        self.set_signature(signature);

        Ok(())
    }
}
