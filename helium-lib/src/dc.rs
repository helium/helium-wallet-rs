use crate::{
    dao::{Dao, SubDao},
    keypair::{Pubkey, PublicKey},
    result::{DecodeError, Result},
    settings::Settings,
    token::{Token, TokenAmount},
};
use anchor_client::solana_sdk::signature::Signer;
use helium_anchor_gen::{circuit_breaker, data_credits};
use std::{ops::Deref, result::Result as StdResult};

pub async fn mint<C: Clone + Deref<Target = impl Signer> + PublicKey>(
    settings: &Settings,
    amount: TokenAmount,
    payee: &Pubkey,
    keypair: C,
) -> Result<solana_sdk::transaction::Transaction> {
    fn token_amount_to_mint_args(
        amount: TokenAmount,
    ) -> StdResult<data_credits::MintDataCreditsArgsV0, DecodeError> {
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

    // let client = self.settings.mk_anchor_client(keypair.clone())?;
    let dc_program = settings
        .mk_anchor_client(keypair.clone())?
        .program(data_credits::id())?;
    let data_credits = SubDao::dc_key();
    let hnt_price_oracle = dc_program
        .account::<data_credits::DataCreditsV0>(data_credits)
        .await?
        .hnt_price_oracle;

    let burner = Token::Hnt.associated_token_adress(&keypair.pubkey());
    let recipient_token_account = Token::Dc.associated_token_adress(payee);
    let accounts = data_credits::accounts::MintDataCreditsV0 {
        data_credits,
        owner: keypair.public_key(),
        hnt_mint: *Token::Hnt.mint(),
        dc_mint: *Token::Dc.mint(),
        recipient: *payee,
        recipient_token_account,
        system_program: solana_sdk::system_program::ID,
        token_program: anchor_spl::token::ID,
        associated_token_program: anchor_spl::associated_token::ID,
        hnt_price_oracle,
        circuit_breaker_program: circuit_breaker::id(),
        circuit_breaker: Token::Dc.mint_circuit_breaker_address(),
        burner,
    };

    let args = data_credits::instruction::MintDataCreditsV0 {
        _args: token_amount_to_mint_args(amount)?,
    };
    let tx = dc_program
        .request()
        .accounts(accounts)
        .args(args)
        .signed_transaction()
        .await?;
    Ok(tx)
}

pub async fn delegate<C: Clone + Deref<Target = impl Signer> + PublicKey>(
    settings: &Settings,
    subdao: SubDao,
    payer_key: &str,
    amount: u64,
    keypair: C,
) -> Result<solana_sdk::transaction::Transaction> {
    let client = settings.mk_anchor_client(keypair.clone())?;
    let dc_program = client.program(data_credits::id())?;

    let delegated_data_credits = subdao.delegated_dc_key(payer_key);

    let accounts = data_credits::accounts::DelegateDataCreditsV0 {
        delegated_data_credits,
        data_credits: SubDao::dc_key(),
        dc_mint: *Token::Dc.mint(),
        dao: Dao::Hnt.key(),
        sub_dao: subdao.key(),
        owner: keypair.public_key(),
        from_account: Token::Dc.associated_token_adress(&keypair.public_key()),
        escrow_account: subdao.escrow_account_key(&delegated_data_credits),
        payer: keypair.public_key(),
        associated_token_program: anchor_spl::associated_token::ID,
        token_program: anchor_spl::token::ID,
        system_program: solana_sdk::system_program::ID,
    };

    let args = data_credits::instruction::DelegateDataCreditsV0 {
        _args: data_credits::DelegateDataCreditsArgsV0 {
            amount,
            router_key: payer_key.to_string(),
        },
    };
    let tx = dc_program
        .request()
        .accounts(accounts)
        .args(args)
        .signed_transaction()
        .await?;
    Ok(tx)
}

pub async fn burn<C: Clone + Deref<Target = impl Signer> + PublicKey>(
    settings: &Settings,
    amount: u64,
    keypair: C,
) -> Result<solana_sdk::transaction::Transaction> {
    let client = settings.mk_anchor_client(keypair.clone())?;
    let dc_program = client.program(data_credits::id())?;

    let accounts = data_credits::accounts::BurnWithoutTrackingV0 {
        BurnWithoutTrackingV0burn_accounts:
            data_credits::accounts::BurnWithoutTrackingV0BurnAccounts {
                burner: Token::Dc.associated_token_adress(&keypair.pubkey()),
                dc_mint: *Token::Dc.mint(),
                data_credits: SubDao::dc_key(),
                token_program: anchor_spl::token::ID,
                system_program: solana_sdk::system_program::ID,
                associated_token_program: anchor_spl::associated_token::ID,
                owner: keypair.public_key(),
            },
    };

    let args = data_credits::instruction::BurnWithoutTrackingV0 {
        _args: data_credits::BurnWithoutTrackingArgsV0 { amount },
    };
    let tx = dc_program
        .request()
        .accounts(accounts)
        .args(args)
        .signed_transaction()
        .await?;
    Ok(tx)
}
