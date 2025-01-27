use crate::{
    anchor_lang::AccountDeserialize,
    anchor_lang::{InstructionData, ToAccountMetas},
    anchor_spl, circuit_breaker,
    client::{GetAnchorAccount, SolanaRpcClient},
    dao::{Dao, SubDao},
    data_credits,
    error::{DecodeError, Error},
    keypair::{Keypair, Pubkey},
    message, priority_fee,
    solana_sdk::{instruction::Instruction, signer::Signer, transaction::VersionedTransaction},
    token::{Token, TokenAmount},
    TransactionOpts,
};
use helium_anchor_gen::{
    data_credits::accounts::BurnDelegatedDataCreditsV0,
    helium_sub_daos::{self, DaoV0, SubDaoV0},
};

pub async fn mint_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: TokenAmount,
    payee: &Pubkey,
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    fn token_amount_to_mint_args(
        amount: TokenAmount,
    ) -> Result<data_credits::MintDataCreditsArgsV0, DecodeError> {
        match amount.token {
            Token::Hnt => Ok(data_credits::MintDataCreditsArgsV0 {
                hnt_amount: Some(amount.amount),
                dc_amount: None,
            }),
            Token::Dc => Ok(data_credits::MintDataCreditsArgsV0 {
                hnt_amount: None,
                dc_amount: Some(amount.amount),
            }),
            other => Err(DecodeError::other(format!("Invalid token type: {other}"))),
        }
    }
    fn mk_accounts(
        owner: &Pubkey,
        recipient: Pubkey,
        hnt_price_oracle: Pubkey,
    ) -> impl ToAccountMetas {
        data_credits::accounts::MintDataCreditsV0 {
            data_credits: Dao::dc_key(),
            owner: *owner,
            hnt_mint: *Token::Hnt.mint(),
            dc_mint: *Token::Dc.mint(),
            recipient,
            recipient_token_account: Token::Dc.associated_token_adress(&recipient),
            system_program: solana_sdk::system_program::ID,
            token_program: anchor_spl::token::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            hnt_price_oracle,
            circuit_breaker_program: circuit_breaker::id(),
            circuit_breaker: Token::Dc.mint_circuit_breaker_address(),
            burner: Token::Hnt.associated_token_adress(owner),
        }
    }

    let hnt_price_oracle = client
        .as_ref()
        .anchor_account::<data_credits::DataCreditsV0>(&Dao::dc_key())
        .await?
        .hnt_price_oracle;

    let ix = Instruction {
        program_id: data_credits::id(),
        accounts: mk_accounts(payer, *payee, hnt_price_oracle).to_account_metas(None),
        data: data_credits::instruction::MintDataCreditsV0 {
            _args: token_amount_to_mint_args(amount)?,
        }
        .data(),
    };

    let ixs = &[
        priority_fee::compute_budget_instruction(300_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &ix.accounts,
            opts.fee_range(),
        )
        .await?,
        ix,
    ];

    message::mk_message(client, ixs, &opts.lut_addresses, payer).await
}

pub async fn mint<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: TokenAmount,
    payee: &Pubkey,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (msg, block_height) = mint_message(client, amount, payee, &keypair.pubkey(), opts).await?;
    let txn = VersionedTransaction::try_new(msg, &[keypair])?;
    Ok((txn, block_height))
}

pub async fn delegate_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    subdao: SubDao,
    payer_key: &str,
    amount: u64,
    owner: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    fn mk_accounts(delegated_dc_key: Pubkey, subdao: SubDao, owner: Pubkey) -> impl ToAccountMetas {
        data_credits::accounts::DelegateDataCreditsV0 {
            delegated_data_credits: delegated_dc_key,
            data_credits: Dao::dc_key(),
            dc_mint: *Token::Dc.mint(),
            dao: Dao::Hnt.key(),
            sub_dao: subdao.key(),
            owner,
            from_account: Token::Dc.associated_token_adress(&owner),
            escrow_account: subdao.escrow_key(&delegated_dc_key),
            payer: owner,
            associated_token_program: anchor_spl::associated_token::ID,
            token_program: anchor_spl::token::ID,
            system_program: solana_sdk::system_program::ID,
        }
    }

    let delegated_dc_key = subdao.delegated_dc_key(payer_key);
    let ix = Instruction {
        program_id: data_credits::id(),
        accounts: mk_accounts(delegated_dc_key, subdao, *owner).to_account_metas(None),
        data: data_credits::instruction::DelegateDataCreditsV0 {
            _args: data_credits::DelegateDataCreditsArgsV0 {
                amount,
                router_key: payer_key.to_string(),
            },
        }
        .data(),
    };

    let ixs = &[
        priority_fee::compute_budget_instruction(150_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &ix.accounts,
            opts.fee_range(),
        )
        .await?,
        ix,
    ];
    message::mk_message(client, ixs, &opts.lut_addresses, owner).await
}

pub async fn delegate<C: AsRef<SolanaRpcClient>>(
    client: &C,
    subdao: SubDao,
    payer_key: &str,
    amount: u64,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (msg, block_height) =
        delegate_message(client, subdao, payer_key, amount, &keypair.pubkey(), opts).await?;
    let txn = VersionedTransaction::try_new(msg, &[keypair])?;
    Ok((txn, block_height))
}

pub async fn burn_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: u64,
    owner: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    fn mk_accounts(owner: Pubkey) -> impl ToAccountMetas {
        data_credits::accounts::BurnWithoutTrackingV0 {
            BurnWithoutTrackingV0burn_accounts:
                data_credits::accounts::BurnWithoutTrackingV0BurnAccounts {
                    burner: Token::Dc.associated_token_adress(&owner),
                    dc_mint: *Token::Dc.mint(),
                    data_credits: Dao::dc_key(),
                    token_program: anchor_spl::token::ID,
                    system_program: solana_sdk::system_program::ID,
                    associated_token_program: anchor_spl::associated_token::ID,
                    owner,
                },
        }
    }

    let ix = Instruction {
        program_id: data_credits::id(),
        accounts: mk_accounts(*owner).to_account_metas(None),
        data: data_credits::instruction::BurnWithoutTrackingV0 {
            _args: data_credits::BurnWithoutTrackingArgsV0 { amount },
        }
        .data(),
    };

    let ixs = &[
        priority_fee::compute_budget_instruction(150_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &ix.accounts,
            opts.fee_range(),
        )
        .await?,
        ix,
    ];
    message::mk_message(client, ixs, &opts.lut_addresses, owner).await
}

pub async fn burn<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: u64,
    keypair: &Keypair,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (msg, block_height) = burn_message(client, amount, &keypair.pubkey(), opts).await?;
    let txn = VersionedTransaction::try_new(msg, &[keypair])?;
    Ok((txn, block_height))
}

pub async fn burn_delegated_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    sub_dao: SubDao,
    amount: u64,
    router_key: Pubkey,
    payer: &Pubkey,
    opts: &TransactionOpts,
) -> Result<(message::VersionedMessage, u64), Error> {
    fn mk_accounts(
        sub_dao: SubDao,
        router_key: Pubkey,
        dc_burn_authority: Pubkey,
        registrar: Pubkey,
    ) -> BurnDelegatedDataCreditsV0 {
        let delegated_data_credits = sub_dao.delegated_dc_key(&router_key.to_string());
        let escrow_account = sub_dao.escrow_key(&delegated_data_credits);

        BurnDelegatedDataCreditsV0 {
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
            helium_sub_daos_program: helium_sub_daos::id(),
            system_program: solana_sdk::system_program::ID,
        }
    }

    let (dc_burn_authority, registrar) = {
        let account_data = client.as_ref().get_account_data(&sub_dao.key()).await?;
        let sub_dao = SubDaoV0::try_deserialize(&mut account_data.as_ref())?;

        let account_data = client.as_ref().get_account_data(&Dao::Hnt.key()).await?;
        let dao = DaoV0::try_deserialize(&mut account_data.as_ref())?;

        (sub_dao.dc_burn_authority, dao.registrar)
    };

    let accounts = mk_accounts(sub_dao, router_key, dc_burn_authority, registrar);
    let burn_ix = solana_sdk::instruction::Instruction {
        program_id: data_credits::id(),
        accounts: accounts.to_account_metas(None),
        data: data_credits::instruction::BurnDelegatedDataCreditsV0 {
            _args: data_credits::BurnDelegatedDataCreditsArgsV0 { amount },
        }
        .data(),
    };

    let ixs = &[
        priority_fee::compute_budget_instruction(150_000),
        priority_fee::compute_price_instruction_for_accounts(
            client,
            &burn_ix.accounts,
            opts.fee_range(),
        )
        .await?,
        burn_ix,
    ];
    message::mk_message(client, ixs, &opts.lut_addresses, payer).await
}

pub async fn burn_delegated<C: AsRef<SolanaRpcClient>>(
    client: &C,
    sub_dao: SubDao,
    keypair: &Keypair,
    amount: u64,
    router_key: Pubkey,
    opts: &TransactionOpts,
) -> Result<(VersionedTransaction, u64), Error> {
    let (msg, block_height) =
        burn_delegated_message(client, sub_dao, amount, router_key, &keypair.pubkey(), opts)
            .await?;
    let txn = VersionedTransaction::try_new(msg, &[keypair])?;
    Ok((txn, block_height))
}
