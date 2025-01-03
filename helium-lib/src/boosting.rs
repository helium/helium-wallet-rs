use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    client::SolanaRpcClient,
    error::Error,
    hexboosting::{self, accounts::StartBoostV0},
    keypair::{Keypair, Pubkey},
    mk_transaction_with_blockhash, priority_fee,
    solana_sdk::{instruction::Instruction, signer::Signer},
    TransactionOpts, TransactionWithBlockhash,
};
use chrono::{DateTime, Utc};

pub trait StartBoostingHex {
    fn start_authority(&self) -> Pubkey;
    fn boost_config(&self) -> Pubkey;
    fn boosted_hex(&self) -> Pubkey;
    fn activation_ts(&self) -> DateTime<Utc>;
}

pub async fn start_boost_transaction<C: AsRef<SolanaRpcClient>>(
    client: &C,
    keypair: &Keypair,
    updates: impl IntoIterator<Item = impl StartBoostingHex>,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    fn mk_accounts(
        start_authority: Pubkey,
        boost_config: Pubkey,
        boosted_hex: Pubkey,
    ) -> impl ToAccountMetas {
        StartBoostV0 {
            start_authority,
            boost_config,
            boosted_hex,
        }
    }

    let mut ix_accounts = vec![];
    let mut start_ixs = vec![];
    for update in updates {
        let accounts = mk_accounts(
            update.start_authority(),
            update.boost_config(),
            update.boosted_hex(),
        );
        let accounts = accounts.to_account_metas(None);
        ix_accounts.extend_from_slice(&accounts);
        let ix = Instruction {
            program_id: hexboosting::id(),
            accounts,
            data: hexboosting::instruction::StartBoostV0 {
                _args: hexboosting::StartBoostArgsV0 {
                    start_ts: update.activation_ts().timestamp(),
                },
            }
            .data(),
        };
        start_ixs.push(ix);
    }
    let ixs = [
        &[
            priority_fee::compute_budget_instruction(150_000),
            priority_fee::compute_price_instruction_for_accounts(
                client,
                &ix_accounts,
                opts.min_priority_fee,
            )
            .await?,
        ],
        start_ixs.as_slice(),
    ]
    .concat();
    mk_transaction_with_blockhash(client, &ixs, &keypair.pubkey()).await
}
pub async fn start_boost<C: AsRef<SolanaRpcClient>>(
    client: &C,
    updates: impl IntoIterator<Item = impl StartBoostingHex>,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<TransactionWithBlockhash, Error> {
    let mut txn = start_boost_transaction(client, keypair, updates, opts).await?;
    txn.try_sign(&[keypair])?;
    Ok(txn)
}
