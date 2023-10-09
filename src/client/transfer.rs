use super::Client;
use crate::{
    keypair::{Pubkey, PublicKey},
    result::Result,
    token::TokenAmount,
};
use anchor_client::solana_sdk::{self, signer::Signer};
use anchor_spl::associated_token::get_associated_token_address;
use std::ops::Deref;

impl Client {
    pub fn transfer<C: Clone + Deref<Target = impl Signer> + PublicKey>(
        &self,
        transfers: &[(Pubkey, TokenAmount)],
        keypair: C,
    ) -> Result<solana_sdk::transaction::Transaction> {
        let client = self.settings.mk_anchor_client(keypair.clone())?;
        let program = client.program(anchor_spl::token::spl_token::id())?;

        let wallet_public_key = keypair.public_key();
        let mut builder = program.request();

        for (payee, token_amount) in transfers {
            let mint_pubkey = token_amount.token.mint();
            let source_pubkey = get_associated_token_address(&wallet_public_key, mint_pubkey);
            let destination_pubkey = get_associated_token_address(payee, mint_pubkey);
            let ix =
                spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                    &wallet_public_key,
                    payee,
                    mint_pubkey,
                    &anchor_spl::token::spl_token::id(),
                );
            builder = builder.instruction(ix);

            let ix = anchor_spl::token::spl_token::instruction::transfer_checked(
                &anchor_spl::token::spl_token::id(),
                &source_pubkey,
                mint_pubkey,
                &destination_pubkey,
                &wallet_public_key,
                &[],
                token_amount.amount,
                token_amount.token.decimals(),
            )?;
            builder = builder.instruction(ix);
        }

        let tx = builder.signed_transaction()?;
        Ok(tx)
    }
}
