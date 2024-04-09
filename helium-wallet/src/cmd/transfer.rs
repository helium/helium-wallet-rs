use crate::cmd::*;
use helium_lib::{
    keypair::{serde_pubkey, Pubkey},
    token::{self, Token, TokenAmount},
};
use serde::Deserialize;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: PayCmd,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, clap::Subcommand)]
/// Send one (or more) payments to given addresses.
///
/// The payment is not submitted to the system unless the '--commit' option is
/// given.
pub enum PayCmd {
    /// Pay a single payee.
    ///
    /// Note that HNT goes to 8 decimals of precision, while MOBILE and
    /// IOT go to 6 decimals of precision
    One(One),
    /// Pay multiple payees
    Multi(Multi),
}

#[derive(Debug, clap::Args)]
pub struct One {
    #[command(flatten)]
    payee: Payee,
    /// Commit the payment to the API
    #[command(flatten)]
    commit: CommitOpts,
}

/// The input file for multiple payments is expected to be json file with a list
/// of payees, amounts, tokens, and optional memos.
/// Notes:
///   "address" is required.
///   "amount" is required. It must be a number or the string "max". When "max"
///            the entire balance (minus fees) will be sent.
///   "token" is optional and defaults to "hnt".
///   "memo" is optional.
///
/// For example:
///
/// [
///     {
///         "address": "<address1>",
///         "amount": 1.6,
///         "memo": "AAAAAAAAAAA=",
///         "token": "hnt"
///     },
///     {
///         "address": "<address2>",
///         "amount": "max"
///     },
///     {
///         "address": "<address3>",
///         "amount": 3,
///         "token": "mobile"
///     }
/// ]
///
#[derive(Debug, clap::Args)]
pub struct Multi {
    /// File to read multiple payments from.
    path: PathBuf,
    /// Commit the payments
    #[command(flatten)]
    commit: CommitOpts,
}

impl PayCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let payments = self.collect_payments()?;
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let settings = opts.try_into()?;

        let tx = token::transfer(&settings, &payments, keypair).await?;

        print_json(&self.commit().maybe_commit(&tx, &settings).await?.to_json())
    }

    fn collect_payments(&self) -> Result<Vec<(Pubkey, TokenAmount)>> {
        match &self {
            Self::One(one) => Ok(vec![(one.payee.address, one.payee.token_amount())]),
            Self::Multi(multi) => {
                let file = std::fs::File::open(multi.path.clone())?;
                let payees: Vec<Payee> = serde_json::from_reader(file)?;
                let payments = payees
                    .iter()
                    .map(|p| (p.address, p.token_amount()))
                    .collect();
                Ok(payments)
            }
        }
    }

    fn commit(&self) -> &CommitOpts {
        match &self {
            Self::One(one) => &one.commit,
            Self::Multi(multi) => &multi.commit,
        }
    }
}

#[derive(Debug, Deserialize, clap::Args)]
pub struct Payee {
    /// Address to send the tokens to.
    #[serde(with = "serde_pubkey")]
    address: Pubkey,
    /// Amount of token to send
    amount: f64,
    /// Type of token to send
    #[arg(value_parser = Token::transferrable_value_parser)]
    token: Token,
}

impl Payee {
    pub fn token_amount(&self) -> TokenAmount {
        TokenAmount::from_f64(self.token, self.amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_hnt_input() {
        let json_hnt_input = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": 1.6,\
            \"token\": \"hnt\"\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).expect("payee");
        assert_eq!(
            TokenAmount {
                amount: 160_000_000,
                token: Token::Hnt
            },
            payee.token_amount()
        );
    }

    #[test]
    fn test_json_mobile_input() {
        let json_hnt_input = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": 0.5,\
            \"token\": \"mobile\"\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).expect("payee");
        assert_eq!(
            TokenAmount {
                amount: 500_000,
                token: Token::Mobile
            },
            payee.token_amount()
        );
    }

    #[test]
    fn test_json_bad_amount() {
        let json_hnt_input = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": \"foo\",\
            \"token\": \"hnt\"\
        }";

        let result: std::result::Result<Payee, serde_json::Error> =
            serde_json::from_str(json_hnt_input);
        assert!(result.is_err());
    }
}
