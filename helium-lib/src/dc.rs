use crate::{
    anchor_lang::{InstructionData, ToAccountMetas},
    anchor_spl, circuit_breaker,
    client::{GetAnchorAccount, SolanaRpcClient},
    dao::Dao,
    data_credits,
    error::{DecodeError, Error},
    keypair::Pubkey,
    message,
    solana_sdk::{instruction::Instruction, signer::Signer},
    token::{Token, TokenAmount},
    transaction::{mk_signed_transaction, VersionedTransaction},
    TransactionOpts,
};

/// Build the bare DC-mint instruction, without the compute-budget framing
/// `mint_message` adds around it.
async fn mint_instruction<C: AsRef<SolanaRpcClient>>(
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

async fn mint_message<C: AsRef<SolanaRpcClient>>(
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
