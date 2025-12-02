use helium_crypto::{PublicKey, Verify};
use msg_signature::MsgHasSignature;

pub trait MsgVerify {
    fn verify(&self, verifier: &PublicKey) -> Result<(), helium_crypto::Error>;
}

impl<T> MsgVerify for T
where
    T: MsgHasSignature + prost::Message,
{
    fn verify(&self, verifier: &PublicKey) -> Result<(), helium_crypto::Error> {
        let msg = self.without_signature();

        let buf = msg.encode_to_vec();
        verifier.verify(&buf, self.signature())?;

        Ok(())
    }
}
