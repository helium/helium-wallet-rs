use helium_crypto::{Keypair, Sign};
use msg_signature::MsgHasSignature;

pub trait MsgSign {
    fn sign(&mut self, signing_key: &Keypair) -> Result<(), helium_crypto::Error>;
}

impl<T> MsgSign for T
where
    T: MsgHasSignature + prost::Message,
{
    fn sign(&mut self, signing_key: &Keypair) -> Result<(), helium_crypto::Error> {
        self.clear_signature();
        let signature = signing_key.sign(&self.encode_to_vec())?;
        self.set_signature(signature);

        Ok(())
    }
}

