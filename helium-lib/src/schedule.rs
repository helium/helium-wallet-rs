use crate::{
    client::{DasClient, GetAnchorAccount, SolanaRpcClient},
    error::Error,
    programs::hpl_crons::{self, accounts::CronJobV0},
    tuktuk_sdk::tuktuk,
    Pubkey,
};

/// Derives the entity cron authority PDA for a wallet.
pub fn entity_cron_authority_key(wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"entity_cron_authority", wallet.as_ref()], &hpl_crons::ID).0
}

/// Derives the cron job PDA for a wallet and job ID.
pub fn cron_job_key_for_wallet(wallet: &Pubkey, job_id: u32) -> Pubkey {
    tuktuk::cron::cron_job_key(&entity_cron_authority_key(wallet), job_id)
}

/// Fetches a cron job account, returning `None` if it does not exist.
pub async fn get<C: AsRef<DasClient> + AsRef<SolanaRpcClient> + GetAnchorAccount>(
    client: &C,
    key: &Pubkey,
) -> Result<Option<CronJobV0>, Error> {
    match client.anchor_account(key).await {
        Ok(acc) => Ok(Some(acc)),
        Err(err) if err.is_account_not_found() => Ok(None),
        Err(err) => Err(err),
    }
}
