use crate::{
    anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas},
    anchor_spl, circuit_breaker,
    client::{GetAnchorAccount, SolanaRpcClient},
    dao::{Dao, SubDao},
    data_credits,
    entity_key::AsEntityKey,
    error::{DecodeError, Error},
    helium_sub_daos,
    keypair::Pubkey,
    message,
    solana_sdk::{instruction::Instruction, signer::Signer},
    token::{Token, TokenAmount},
    transaction::{mk_signed_transaction, VersionedTransaction},
    TransactionOpts,
};

/// Build the bare DC-mint instruction (no compute-budget framing). Used
/// by both `mint_message` and Squads-mode wrappers; same args/accounts,
/// just stripped of the outer envelope.
pub async fn mint_instruction<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: TokenAmount,
    payee: &Pubkey,
    payer: &Pubkey,
) -> Result<Instruction, Error> {
    let mint_args = match amount.token {
        Token::Hnt => data_credits::types::MintDataCreditsArgsV0 {
            hnt_amount: Some(amount.amount),
            dc_amount: None,
        },
        Token::Dc => data_credits::types::MintDataCreditsArgsV0 {
            hnt_amount: None,
            dc_amount: Some(amount.amount),
        },
        other => {
            return Err(DecodeError::other(format!("Invalid token type: {other}")).into());
        }
    };
    let hnt_price_oracle = client
        .as_ref()
        .anchor_account::<data_credits::accounts::DataCreditsV0>(&Dao::dc_key())
        .await?
        .hnt_price_oracle;
    let accounts = data_credits::client::accounts::MintDataCreditsV0 {
        data_credits: Dao::dc_key(),
        owner: *payer,
        hnt_mint: *Token::Hnt.mint(),
        dc_mint: *Token::Dc.mint(),
        recipient: *payee,
        recipient_token_account: Token::Dc.associated_token_address(payee),
        system_program: solana_sdk::system_program::ID,
        token_program: anchor_spl::token::ID,
        associated_token_program: anchor_spl::associated_token::ID,
        hnt_price_oracle,
        circuit_breaker_program: circuit_breaker::ID,
        circuit_breaker: Token::Dc.mint_circuit_breaker_address(),
        burner: Token::Hnt.associated_token_address(payer),
    };
    Ok(Instruction {
        program_id: data_credits::ID,
        accounts: accounts.to_account_metas(None),
        data: data_credits::client::args::MintDataCreditsV0 { args: mint_args }.data(),
    })
}

pub async fn mint_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: TokenAmount,
    payee: &Pubkey,
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let ix = mint_instruction(client, amount, payee, payer).await?;
    message::mk_budgeted_message(client, 300_000, &[ix], payer, opts).await
}

/// Mints data credits by burning HNT and returns a signed transaction.
pub async fn mint<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: TokenAmount,
    payee: &Pubkey,
    keypair: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let msg = mint_message(client, amount, payee, &keypair.pubkey(), opts).await?;
    mk_signed_transaction(msg, &[keypair])
}

/// Build the bare DC-delegate instruction (no compute-budget framing).
/// Used by both `delegate_message` and Squads-mode wrappers.
pub fn delegate_instruction(
    subdao: SubDao,
    payer_key: &str,
    amount: u64,
    owner: &Pubkey,
) -> Instruction {
    let delegated_dc_key = subdao.delegated_dc_key(&payer_key);
    let accounts = data_credits::client::accounts::DelegateDataCreditsV0 {
        delegated_data_credits: delegated_dc_key,
        data_credits: Dao::dc_key(),
        dc_mint: *Token::Dc.mint(),
        dao: Dao::Hnt.key(),
        sub_dao: subdao.key(),
        owner: *owner,
        from_account: Token::Dc.associated_token_address(owner),
        escrow_account: subdao.escrow_key(&delegated_dc_key),
        payer: *owner,
        associated_token_program: anchor_spl::associated_token::ID,
        token_program: anchor_spl::token::ID,
        system_program: solana_sdk::system_program::ID,
    };
    Instruction {
        program_id: data_credits::ID,
        accounts: accounts.to_account_metas(None),
        data: data_credits::client::args::DelegateDataCreditsV0 {
            args: data_credits::types::DelegateDataCreditsArgsV0 {
                amount,
                router_key: payer_key.to_string(),
            },
        }
        .data(),
    }
}

/// Builds a message that delegates data credits to a router/OUI.
pub async fn delegate_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    subdao: SubDao,
    payer_key: &str,
    amount: u64,
    owner: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let ix = delegate_instruction(subdao, payer_key, amount, owner);
    message::mk_budgeted_message(client, 150_000, &[ix], owner, opts).await
}

/// Delegates data credits to a router/OUI and returns a signed transaction.
pub async fn delegate<C: AsRef<SolanaRpcClient>>(
    client: &C,
    subdao: SubDao,
    payer_key: &str,
    amount: u64,
    keypair: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let msg = delegate_message(client, subdao, payer_key, amount, &keypair.pubkey(), opts).await?;
    mk_signed_transaction(msg, &[keypair])
}

/// Build the bare DC-burn instruction (no compute-budget framing).
/// Used by both `burn_message` and Squads-mode wrappers.
pub fn burn_instruction(amount: u64, owner: &Pubkey) -> Instruction {
    let accounts = data_credits::client::accounts::BurnWithoutTrackingV0 {
        burn_accounts: data_credits::client::accounts::BurnAccounts {
            burner: Token::Dc.associated_token_address(owner),
            dc_mint: *Token::Dc.mint(),
            data_credits: Dao::dc_key(),
            token_program: anchor_spl::token::ID,
            system_program: solana_sdk::system_program::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            owner: *owner,
        },
    };
    Instruction {
        program_id: data_credits::ID,
        accounts: accounts.to_account_metas(None),
        data: data_credits::client::args::BurnWithoutTrackingV0 {
            args: data_credits::types::BurnWithoutTrackingArgsV0 { amount },
        }
        .data(),
    }
}

/// Builds a message that burns data credits without tracking.
pub async fn burn_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: u64,
    owner: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    let ix = burn_instruction(amount, owner);
    message::mk_budgeted_message(client, 150_000, &[ix], owner, opts).await
}

/// Burns data credits and returns a signed transaction.
pub async fn burn<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: u64,
    keypair: &(dyn Signer + Sync),
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let msg = burn_message(client, amount, &keypair.pubkey(), opts).await?;
    mk_signed_transaction(msg, &[keypair])
}

/// Builds a message that burns delegated data credits for a router.
pub async fn burn_delegated_message<C: AsRef<SolanaRpcClient>, E: AsEntityKey>(
    client: &C,
    sub_dao: SubDao,
    amount: u64,
    router_key: &E,
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    fn mk_accounts<E: AsEntityKey>(
        sub_dao: SubDao,
        router_key: &E,
        dc_burn_authority: Pubkey,
        registrar: Pubkey,
    ) -> impl ToAccountMetas {
        let delegated_data_credits = sub_dao.delegated_dc_key(router_key);
        let escrow_account = sub_dao.escrow_key(&delegated_data_credits);

        data_credits::client::accounts::BurnDelegatedDataCreditsV0 {
            sub_dao_epoch_info: sub_dao.epoch_info_key(),
            delegated_data_credits,
            escrow_account,

            dao: Dao::Hnt.key(),
            sub_dao: sub_dao.key(),

            account_payer: Dao::dc_account_payer(),
            data_credits: Dao::dc_key(),
            dc_burn_authority,
            dc_mint: *Token::Dc.mint(),
            registrar,

            token_program: anchor_spl::token::ID,
            helium_sub_daos_program: helium_sub_daos::ID,
            system_program: solana_sdk::system_program::ID,
        }
    }

    let (dc_burn_authority, registrar) = {
        let account_data = client.as_ref().get_account_data(&sub_dao.key()).await?;
        let sub_dao =
            helium_sub_daos::accounts::SubDaoV0::try_deserialize(&mut account_data.as_ref())?;

        let account_data = client.as_ref().get_account_data(&Dao::Hnt.key()).await?;
        let dao = helium_sub_daos::accounts::DaoV0::try_deserialize(&mut account_data.as_ref())?;

        (sub_dao.dc_burn_authority, dao.registrar)
    };

    let accounts = mk_accounts(sub_dao, router_key, dc_burn_authority, registrar);
    let burn_ix = solana_sdk::instruction::Instruction {
        program_id: data_credits::ID,
        accounts: accounts.to_account_metas(None),
        data: data_credits::client::args::BurnDelegatedDataCreditsV0 {
            args: data_credits::types::BurnDelegatedDataCreditsArgsV0 { amount },
        }
        .data(),
    };

    message::mk_budgeted_message(client, 150_000, &[burn_ix], payer, opts).await
}

/// Burns delegated data credits and returns a signed transaction.
pub async fn burn_delegated<C: AsRef<SolanaRpcClient>, E: AsEntityKey>(
    client: &C,
    sub_dao: SubDao,
    keypair: &(dyn Signer + Sync),
    amount: u64,
    router_key: &E,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let msg = burn_delegated_message(client, sub_dao, amount, router_key, &keypair.pubkey(), opts)
        .await?;
    mk_signed_transaction(msg, &[keypair])
}
