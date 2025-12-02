use helium_crypto::{PublicKey, Verify};
use msg_signature::MsgHasSignature;

#[derive(thiserror::Error, Debug)]
pub enum MsgVerifyError {
    #[error("prost encode error: {0}")]
    Prost(#[from] prost::EncodeError),

    #[error("crypto error: {0}")]
    Crypto(#[from] helium_crypto::Error),
}

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
