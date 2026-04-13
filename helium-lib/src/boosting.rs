use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    client::SolanaRpcClient,
    error::Error,
    hexboosting,
    keypair::{Keypair, Pubkey},
    message, priority_fee,
    solana_sdk::{instruction::Instruction, signer::Signer},
    transaction, TransactionOpts,
};
use chrono::{DateTime, Utc};

/// Parameters required to activate hex boosting for a mobile hotspot.
pub trait StartBoostingHex {
    /// The authority that can start boosting.
    fn start_authority(&self) -> Pubkey;
    /// The boost configuration account.
    fn boost_config(&self) -> Pubkey;
    /// The boosted hex account.
    fn boosted_hex(&self) -> Pubkey;
    /// When the boost should become active.
    fn activation_ts(&self) -> DateTime<Utc>;
}

/// Builds a message that activates hex boosting for a set of hexes.
pub async fn start_boost_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    keypair: &Keypair,
    updates: impl IntoIterator<Item = impl StartBoostingHex>,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    fn mk_accounts(
        start_authority: Pubkey,
        boost_config: Pubkey,
        boosted_hex: Pubkey,
    ) -> impl ToAccountMetas {
        hexboosting::client::accounts::StartBoostV0 {
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
            program_id: hexboosting::ID,
            accounts,
            data: hexboosting::client::args::StartBoostV0 {
                args: hexboosting::types::StartBoostArgsV0 {
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
                opts.fee_range(),
            )
            .await?,
        ],
        start_ixs.as_slice(),
    ]
    .concat();
    message::mk_message(client, &ixs, &opts.lut_addresses, &keypair.pubkey()).await
}

/// Activate hex boosting and return a signed transaction.
pub async fn start_boost<C: AsRef<SolanaRpcClient>>(
    client: &C,
    updates: impl IntoIterator<Item = impl StartBoostingHex>,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(transaction::VersionedTransaction, u64), Error> {
    let (msg, block_height) = start_boost_message(client, keypair, updates, opts).await?;
    let txn = transaction::mk_transaction(msg, &[keypair])?;
    Ok((txn, block_height))
}
