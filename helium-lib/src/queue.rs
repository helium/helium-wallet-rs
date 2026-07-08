use crate::{solana_sdk::pubkey, Pubkey};
use tuktuk_sdk::tuktuk;

/// Well-known task queue address for reward claims.
pub const TASK_QUEUE_ID: Pubkey = pubkey!("H39gEszvsi6AT4rYBiJTuZHJSF5hMHy6CKGTd7wzhsg7");

/// Derives the PDA for a wallet's claim payer signer.
pub fn claim_wallet_key(task_queue_key: &Pubkey, wallet: &Pubkey) -> Pubkey {
    tuktuk::custom_signer_key(task_queue_key, &[b"claim_payer", wallet.as_ref()])
}

/// Derives the task queue authority PDA.
pub fn task_queue_authority_key(task_queue_key: &Pubkey, queue_authority: &Pubkey) -> Pubkey {
    tuktuk::task_queue::task_queue_authority_key(task_queue_key, queue_authority)
}
