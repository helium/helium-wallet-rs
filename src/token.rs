use crate::keypair::Pubkey;
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("DC can not be represented with decimals")]
    InvalidDCConversion,
    #[error("{0}")]
    ProgramRpcError(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("Invalid token type: {0}")]
    InvalidToken(String),
}

lazy_static::lazy_static! {
    static ref HNT_MINT: Pubkey = Pubkey::from_str("hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux").unwrap();
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
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|_| TokenError::InvalidToken(s.to_string()))
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
}

pub struct TokenAmount {
    pub token: Token,
    pub amount: u64,
    pub decimals: u8,
}

impl TryFrom<&TokenAmount> for f64 {
    type Error = TokenError;
    fn try_from(value: &TokenAmount) -> std::result::Result<Self, Self::Error> {
        Ok(value.amount as f64 / 10_usize.pow(value.decimals as u32) as f64)
    }
}

impl serde::Serialize for TokenAmount {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::Error;
        match self.token {
            Token::Sol | Token::Hnt | Token::Iot | Token::Mobile => {
                serializer.serialize_f64(self.try_into().map_err(S::Error::custom)?)
            }
            Token::Dc => serializer.serialize_u64(self.amount),
        }
    }
}

impl Default for TokenAmount {
    fn default() -> Self {
        Self {
            token: Token::Dc,
            amount: 0,
            decimals: 0,
        }
    }
}

impl Token {
    // pub async fn get_balance_for_account(
    //     &self,
    //     client: Arc<RpcClient>,
    //     pubkey: &Pubkey,
    // ) -> Result<TokenAmount> {
    //     match self {
    //         Self::Sol => {
    //             let account = client.get_account(pubkey).await?;
    //             Ok(self.to_balance(account.lamports))
    //         }
    //         _ => {
    //             let spl_atc =
    //                 spl_associated_token_account::get_associated_token_address(pubkey, self.mint());
    //             self.get_balance_for_address(client, spl_atc).await
    //         }
    //     }
    // }

    // pub async fn get_balance_for_address(
    //     &self,
    //     client: Arc<RpcClient>,
    //     pubkey: Pubkey,
    // ) -> Result<TokenAmount> {
    //     let program_rpc_client = ProgramRpcClient::new(client, ProgramRpcClientSendTransaction);

    //     match program_rpc_client
    //         .get_account(pubkey)
    //         .await
    //         .map_err(TokenError::from)?
    //     {
    //         Some(account) => {
    //             let account_data =
    //                 StateWithExtensions::<spl_token_2022::state::Account>::unpack(&account.data)?;
    //             Ok(self.to_balance(account_data.base.amount))
    //         }
    //         None => Ok(self.to_balance(0)),
    //     }
    // }

    pub fn mint(&self) -> &Pubkey {
        match self {
            Self::Hnt => &HNT_MINT,
            Self::Mobile => &MOBILE_MINT,
            Self::Iot => &IOT_MINT,
            Self::Dc => &DC_MINT,
            Self::Sol => &SOL_MINT,
        }
    }

    pub fn to_balance(&self, amount: u64, decimals: u8) -> TokenAmount {
        TokenAmount {
            token: *self,
            amount,
            decimals,
        }
    }
}
