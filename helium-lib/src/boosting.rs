use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    client::SolanaRpcClient,
    error::Error,
    hexboosting,
    keypair::Pubkey,
    message,
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
    keypair: &(dyn Signer + Sync),
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

    let mut start_ixs = vec![];
    for update in updates {
        let accounts = mk_accounts(
            update.start_authority(),
            update.boost_config(),
            update.boosted_hex(),
        );
        let ix = Instruction {
            program_id: hexboosting::ID,
            accounts: accounts.to_account_metas(None),
            data: hexboosting::client::args::StartBoostV0 {
                args: hexboosting::types::StartBoostArgsV0 {
                    start_ts: update.activation_ts().timestamp(),
                },
            }
            .data(),
        };
        start_ixs.push(ix);
    }
    message::mk_budgeted_message(client, 150_000, &start_ixs, &keypair.pubkey(), opts).await
}

/// Activate hex boosting and return a signed transaction.
pub async fn start_boost<C: AsRef<SolanaRpcClient>>(
    client: &C,
    updates: impl IntoIterator<Item = impl StartBoostingHex>,
    keypair: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(transaction::VersionedTransaction, u64), Error> {
    let msg = start_boost_message(client, keypair, updates, opts).await?;
    transaction::mk_signed_transaction(msg, &[keypair])
}
