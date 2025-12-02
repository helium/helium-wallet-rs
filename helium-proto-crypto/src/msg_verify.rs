use helium_crypto::{PublicKey, Verify};
use msg_signature::MsgHasSignature;

#[derive(thiserror::Error, Debug)]
#[error("error verifying signature: {0}")]
pub struct MsgVerifyError(#[from] pub helium_crypto::Error);

pub trait MsgVerify {
    fn verify(&self, verifier: &PublicKey) -> Result<(), MsgVerifyError>;
}

impl<T> MsgVerify for T
where
    T: MsgHasSignature + prost::Message,
{
    fn verify(&self, verifier: &PublicKey) -> Result<(), MsgVerifyError> {
        let msg = self.without_signature();

        let buf = msg.encode_to_vec();
        verifier.verify(&buf, self.signature())?;

        Ok(())
    }
}
