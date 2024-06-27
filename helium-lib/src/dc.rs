use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    anchor_spl, circuit_breaker,
    client::{GetAnchorAccount, SolanaRpcClient},
    dao::{Dao, SubDao},
    data_credits,
    error::{DecodeError, Error},
    keypair::{Keypair, Pubkey},
    solana_sdk::{instruction::Instruction, signer::Signer, transaction::Transaction},
    token::{Token, TokenAmount},
};

pub async fn mint<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: TokenAmount,
    payee: &Pubkey,
    keypair: &Keypair,
) -> Result<Transaction, Error> {
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
        owner: Pubkey,
        recipient: Pubkey,
        hnt_price_oracle: Pubkey,
    ) -> impl ToAccountMetas {
        data_credits::accounts::MintDataCreditsV0 {
            data_credits: SubDao::dc_key(),
            owner,
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
            burner: Token::Hnt.associated_token_adress(&owner),
        }
    }

    let hnt_price_oracle = client
        .as_ref()
        .anchor_account::<data_credits::DataCreditsV0>(&SubDao::dc_key())
        .await?
        .hnt_price_oracle;

    let mint_ix = Instruction {
        program_id: data_credits::id(),
        accounts: mk_accounts(keypair.pubkey(), *payee, hnt_price_oracle).to_account_metas(None),
        data: data_credits::instruction::MintDataCreditsV0 {
            _args: token_amount_to_mint_args(amount)?,
        }
        .data(),
    };

    let recent_blockhash = client.as_ref().get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[mint_ix],
        Some(&keypair.pubkey()),
        &[keypair],
        recent_blockhash,
    );

    Ok(tx)
}

pub async fn delegate<C: AsRef<SolanaRpcClient>>(
    client: &C,
    subdao: SubDao,
    payer_key: &str,
    amount: u64,
    keypair: &Keypair,
) -> Result<solana_sdk::transaction::Transaction, Error> {
    fn mk_accounts(delegated_dc_key: Pubkey, subdao: SubDao, owner: Pubkey) -> impl ToAccountMetas {
        data_credits::accounts::DelegateDataCreditsV0 {
            delegated_data_credits: delegated_dc_key,
            data_credits: SubDao::dc_key(),
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
    let delegate_ix = Instruction {
        program_id: data_credits::id(),
        accounts: mk_accounts(delegated_dc_key, subdao, keypair.pubkey()).to_account_metas(None),
        data: data_credits::instruction::DelegateDataCreditsV0 {
            _args: data_credits::DelegateDataCreditsArgsV0 {
                amount,
                router_key: payer_key.to_string(),
            },
        }
        .data(),
    };
    let recent_blockhash = client.as_ref().get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[delegate_ix],
        Some(&keypair.pubkey()),
        &[keypair],
        recent_blockhash,
    );
    Ok(tx)
}

pub async fn burn<C: AsRef<SolanaRpcClient>>(
    client: &C,
    amount: u64,
    keypair: &Keypair,
) -> Result<solana_sdk::transaction::Transaction, Error> {
    fn mk_accounts(owner: Pubkey) -> impl ToAccountMetas {
        data_credits::accounts::BurnWithoutTrackingV0 {
            BurnWithoutTrackingV0burn_accounts:
                data_credits::accounts::BurnWithoutTrackingV0BurnAccounts {
                    burner: Token::Dc.associated_token_adress(&owner),
                    dc_mint: *Token::Dc.mint(),
                    data_credits: SubDao::dc_key(),
                    token_program: anchor_spl::token::ID,
                    system_program: solana_sdk::system_program::ID,
                    associated_token_program: anchor_spl::associated_token::ID,
                    owner,
                },
        }
    }

    let burn_ix = Instruction {
        program_id: data_credits::id(),
        accounts: mk_accounts(keypair.pubkey()).to_account_metas(None),
        data: data_credits::instruction::BurnWithoutTrackingV0 {
            _args: data_credits::BurnWithoutTrackingArgsV0 { amount },
        }
        .data(),
    };
    let recent_blockhash = client.as_ref().get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(
        &[burn_ix],
        Some(&keypair.pubkey()),
        &[keypair],
        recent_blockhash,
    );
    Ok(tx)
}
