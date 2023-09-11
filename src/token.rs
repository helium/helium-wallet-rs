use crate::keypair::{serde_pubkey, Pubkey};
use std::{result::Result as StdResult, str::FromStr};

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Invalid token type: {0}")]
    InvalidToken(String),
}

lazy_static::lazy_static! {
    static ref HNT_MINT: Pubkey = Pubkey::from_str("hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux").unwrap();
    static ref HNT_PRICE_KEY: Pubkey = Pubkey::from_str("7moA1i5vQUpfDwSpK6Pw9s56ahB7WFGidtbL2ujWrVvm").unwrap();

    static ref MOBILE_MINT: Pubkey = Pubkey::from_str("mb1eu7TzEc71KxDpsmsKoucSSuuoGLv1drys1oP2jh6").unwrap();
    static ref IOT_MINT: Pubkey = Pubkey::from_str("iotEVVZLEywoTn1QdwNPddxPWszn3zFhEot3MfL9fns").unwrap();
    static ref DC_MINT: Pubkey = Pubkey::from_str("dcuc8Amr83Wz27ZkQ2K9NS6r8zRpf1J6cvArEBDZDmm").unwrap();
    static ref SOL_MINT: Pubkey = anchor_spl::token::ID;
}

#[derive(
    Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Copy, clap::ValueEnum, Hash,
)]
#[serde(rename_all = "lowercase")]
pub enum Token {
    Sol,
    Hnt,
    Mobile,
    Iot,
    Dc,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&str)
    }
}

impl std::str::FromStr for Token {
    type Err = TokenError;
    fn from_str(s: &str) -> StdResult<Self, Self::Err> {
        serde_json::from_str(&format!("\"{s}\""))
            .map_err(|_| TokenError::InvalidToken(s.to_string()))
    }
}

impl Token {
    pub fn from_mint(mint: Pubkey) -> Option<Self> {
        let token = match mint {
            mint if mint == *HNT_MINT => Token::Hnt,
            mint if mint == *IOT_MINT => Token::Iot,
            mint if mint == *DC_MINT => Token::Dc,
            mint if mint == *MOBILE_MINT => Token::Mobile,
            mint if mint == *SOL_MINT => Token::Sol,
            _ => return None,
        };

        Some(token)
    }

    pub fn all() -> Vec<Self> {
        vec![Self::Hnt, Self::Iot, Self::Mobile, Self::Dc, Self::Sol]
    }

    pub(crate) fn transferrable_value_parser(s: &str) -> Result<Self, TokenError> {
        let transferrable = [Self::Iot, Self::Mobile, Self::Hnt, Self::Sol];
        let result = Self::from_str(s)?;
        if !transferrable.contains(&result) {
            return Err(TokenError::InvalidToken(s.to_string()));
        }
        Ok(result)
    }

    pub fn associated_token_adress(&self, address: &Pubkey) -> Pubkey {
        match self {
            Self::Sol => *address,
            _ => spl_associated_token_account::get_associated_token_address(address, self.mint()),
        }
    }

    pub fn associated_token_adresses(address: &Pubkey) -> Vec<Pubkey> {
        Self::all()
            .iter()
            .map(|token| token.associated_token_adress(address))
            .collect::<Vec<_>>()
    }
}

#[derive(Debug, serde::Serialize)]
pub struct TokenBalance {
    #[serde(with = "serde_pubkey")]
    pub address: Pubkey,
    pub amount: TokenAmount,
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

impl Token {
    pub fn decimals(&self) -> u8 {
        match self {
            Self::Hnt => 8,
            Self::Iot | Self::Mobile => 6,
            Self::Dc => 0,
            Self::Sol => 9,
        }
    }

    pub fn mint(&self) -> &Pubkey {
        match self {
            Self::Hnt => &HNT_MINT,
            Self::Mobile => &MOBILE_MINT,
            Self::Iot => &IOT_MINT,
            Self::Dc => &DC_MINT,
            Self::Sol => &SOL_MINT,
        }
    }

    pub fn price_key(&self) -> Option<&Pubkey> {
        match self {
            Self::Hnt => Some(&HNT_PRICE_KEY),
            _ => None,
        }
    }

    pub fn to_balance(self, address: Pubkey, amount: u64) -> TokenBalance {
        TokenBalance {
            address,
            amount: TokenAmount::from_u64(self, amount),
        }
    }
}
