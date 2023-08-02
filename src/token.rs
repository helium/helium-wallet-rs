use crate::keypair::{serde_pubkey, Pubkey};
use hpl_utils::token::Token;

#[derive(Debug, serde::Serialize)]
pub struct TokenBalance {
    #[serde(with = "serde_pubkey")]
    pub address: Pubkey,
    pub amount: TokenAmount,
}

impl TokenBalance {
    pub fn from_token(token: Token, address: Pubkey, amount: u64) -> Self {
        Self {
            address,
            amount: TokenAmount::from_u64(token, amount),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TokenAmount {
    pub token: Token,
    pub amount: u64,
}

impl From<&TokenAmount> for f64 {
    fn from(value: &TokenAmount) -> Self {
        match value.token.decimals() {
            0 => value.amount as f64,
            decimals => value.amount as f64 / 10_usize.pow(decimals.into()) as f64,
        }
    }
}

impl serde::Serialize for TokenAmount {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.token.decimals() == 0 {
            serializer.serialize_u64(self.amount)
        } else {
            serializer.serialize_f64(self.into())
        }
    }
}

impl Default for TokenAmount {
    fn default() -> Self {
        Self {
            token: Token::Dc,
            amount: 0,
        }
    }
}

impl TokenAmount {
    pub fn from_f64(token: Token, amount: f64) -> Self {
        let amount = (amount * 10_usize.pow(token.decimals().into()) as f64) as u64;
        Self { token, amount }
    }

    pub fn from_u64(token: Token, amount: u64) -> Self {
        Self { token, amount }
    }
}
