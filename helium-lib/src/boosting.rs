use chrono::{DateTime, Utc};
use helium_anchor_gen::hexboosting::accounts::StartBoostV0;

use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    client::SolanaRpcClient,
    error::Error,
    hexboosting,
    keypair::Keypair,
    keypair::Pubkey,
    solana_sdk::{instruction::Instruction, signature::Signer, transaction::Transaction},
};

pub trait StartBoostingHex {
    fn start_authority(&self) -> Pubkey;
    fn boost_config(&self) -> Pubkey;
    fn boosted_hex(&self) -> Pubkey;
    fn activation_ts(&self) -> DateTime<Utc>;
}

pub async fn start_boost<C: AsRef<SolanaRpcClient>>(
    client: &C,
    keypair: &Keypair,
    updates: impl IntoIterator<Item = impl StartBoostingHex>,
) -> Result<Transaction, Error> {
    fn mk_accounts(
        start_authority: Pubkey,
        boost_config: Pubkey,
        boosted_hex: Pubkey,
    ) -> StartBoostV0 {
        StartBoostV0 {
            start_authority,
            boost_config,
            boosted_hex,
        }
    }

    let mut ixs = vec![];
    for update in updates {
        let accounts = mk_accounts(
            update.start_authority(),
            update.boost_config(),
            update.boosted_hex(),
        );
        let ix = Instruction {
            program_id: hexboosting::id(),
            accounts: accounts.to_account_metas(None),
            data: hexboosting::instruction::StartBoostV0 {
                _args: hexboosting::StartBoostArgsV0 {
                    start_ts: update.activation_ts().timestamp(),
                },
            }
            .data(),
        };
        ixs.push(ix);
    }

    let recent_blockhash = client.as_ref().get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&keypair.pubkey()),
        &[keypair],
        recent_blockhash,
    );

    Ok(tx)
}
