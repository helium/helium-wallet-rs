use crate::{cmd::*, result::Result};
use bulk_claim_rewards::{claim_rewards::claim_rewards_blocking, claim_rewards::ClaimRewardsArgs};
use hpl_utils::dao::{Dao, SubDao};
use solana_program::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Instant;

#[derive(Clone, Debug, clap::Args)]
pub struct Cmd {
    #[arg(long)]
    hotspot_owner: Option<String>,
    /// Type of rewards to claim, either 'iot' or 'mobile'
    #[arg(long)]
    sub_dao: SubDao,
    /// Number of NFTs to check at a time. Defaults to 100
    #[arg(long, default_value = "100")]
    batch_size: usize,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let rewards_mint = self.sub_dao.mint();
        let dao = Dao::Hnt.key();

        let start = Instant::now();
        claim_rewards_blocking(ClaimRewardsArgs {
            rpc_url: client.settings.to_string().as_str(),
            payer: keypair.clone(),
            hotspot_owner: self
                .hotspot_owner
                .clone()
                .map(|p| Pubkey::from_str(&p).unwrap())
                .unwrap_or_else(|| keypair.public_key()),
            rewards_mint: *rewards_mint,
            dao,
            batch_size: self.batch_size,
        })
        .unwrap();
        let duration = start.elapsed();
        println!("Time elapsed is: {:?}", duration);
        Ok(())
    }
}
