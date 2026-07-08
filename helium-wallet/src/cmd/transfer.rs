use crate::{
    cmd::{squads::SquadsOpts, *},
    result::Result,
};
use helium_lib::{
    blockchain_api::types::{
        MultiTransferRequest, Recipient, TokenAmountInput, TokenTransferRequest,
    },
    keypair::{serde_pubkey, Pubkey, Signer},
    token::{Token, TokenAmount},
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
/// Send one (or more) HNT payments to given addresses.
///
/// The payment is not submitted to the system unless the '--commit' option is
/// given.
pub enum PayCmd {
    /// Pay a single payee in HNT (8 decimals of precision).
    One(One),
    /// Pay multiple payees in HNT
    Multi(Multi),
}

#[derive(Debug, clap::Args)]
pub struct One {
    #[command(flatten)]
    payee: Payee,
    #[command(flatten)]
    squads: SquadsOpts,
    /// Commit the payment to the API
    #[command(flatten)]
    commit: CommitOpts,
}

/// Multiple payment descriptor file
///
/// The input file for multiple payments is expected to be a json file with a
/// list of payees and amounts. All payments are in HNT.
/// Notes:
///   "address" is required.
///   "amount" is required and must be a positive number.
///
/// For example:
///
/// [
///     {
///         "address": "<address1>",
///         "amount": 1.6
///     },
///     {
///         "address": "<address2>",
///         "amount": 3
///     }
/// ]
///
#[derive(Debug, clap::Args)]
pub struct Multi {
    /// File to read multiple payments from.
    path: PathBuf,
    #[command(flatten)]
    squads: SquadsOpts,
    /// Commit the payments
    #[command(flatten)]
    commit: CommitOpts,
}

impl PayCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let payments = self.collect_payments()?;
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let squads = self.squads();

        // Transfers build via the blockchain-api. A single recipient goes
        // through tokens/transfer (as a Squads vault proposal when --squads is
        // set); multiple recipients go through the single-mint multi-transfer
        // endpoint. --squads supports a single recipient only. All transfers
        // are HNT.
        let api = opts.blockchain_api()?;
        let wallet_address = signer.pubkey().to_string();
        let response = match payments.as_slice() {
            [(destination, amount)] => {
                let (multisig, memo) = squads.resolve(&client, &signer.pubkey()).await?;
                api.token_transfer(&TokenTransferRequest {
                    wallet_address,
                    destination: destination.to_string(),
                    token_amount: TokenAmountInput::new(amount.token.mint(), amount.amount),
                    multisig,
                    memo,
                })
                .await?
            }
            recipients => {
                if squads.squads.is_some() {
                    bail!("--squads supports a single recipient only");
                }
                let recipients = recipients
                    .iter()
                    .map(|(destination, amount)| Recipient {
                        destination: destination.to_string(),
                        amount: amount.amount.to_string(),
                    })
                    .collect();
                api.multi_transfer(&MultiTransferRequest {
                    wallet_address,
                    mint: Token::Hnt.mint().to_string(),
                    recipients,
                })
                .await?
            }
        };
        print_json(
            &self
                .commit()
                .commit_via_api(
                    &api,
                    &client,
                    &response,
                    &*signer,
                    ApiSigning::FreshBlockhash,
                )
                .await?
                .to_json(),
        )
    }

    fn squads(&self) -> &SquadsOpts {
        match &self {
            Self::One(one) => &one.squads,
            Self::Multi(multi) => &multi.squads,
        }
    }

    fn collect_payments(&self) -> Result<Vec<(Pubkey, TokenAmount)>> {
        match &self {
            Self::One(one) => Ok(vec![(one.payee.address, one.payee.token_amount()?)]),
            Self::Multi(multi) => {
                let file = std::fs::File::open(multi.path.clone())?;
                let payees: Vec<Payee> = serde_json::from_reader(file)?;
                payees
                    .iter()
                    .map(|p| Ok((p.address, p.token_amount()?)))
                    .collect()
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

// deny_unknown_fields so a typo'd or unsupported key in a multi-payment
// file (e.g. "token", dropped when transfers became HNT-only, or "memo",
// which the old doc advertised but was silently dropped) is an error
// instead of being ignored.
#[derive(Debug, Deserialize, clap::Args)]
#[serde(deny_unknown_fields)]
pub struct Payee {
    /// Address to send the HNT to.
    #[serde(with = "serde_pubkey")]
    address: Pubkey,
    /// Amount of HNT to send
    amount: f64,
}

impl Payee {
    pub fn token_amount(&self) -> Result<TokenAmount> {
        Ok(TokenAmount::from_f64(Token::Hnt, self.amount)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_hnt_input() {
        let json_hnt_input = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": 1.6\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).expect("payee");
        assert_eq!(
            TokenAmount {
                amount: 160_000_000,
                token: Token::Hnt
            },
            payee.token_amount().expect("token amount")
        );
    }

    #[test]
    fn test_json_rejects_token_field() {
        // Transfers are HNT-only; a leftover "token" key must be rejected
        // rather than silently ignored (deny_unknown_fields).
        let json_with_token = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": 0.5,\
            \"token\": \"mobile\"\
        }";

        let result: std::result::Result<Payee, serde_json::Error> =
            serde_json::from_str(json_with_token);
        assert!(result.is_err());
    }

    #[test]
    fn test_json_bad_amount() {
        let json_hnt_input = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": \"foo\"\
        }";

        let result: std::result::Result<Payee, serde_json::Error> =
            serde_json::from_str(json_hnt_input);
        assert!(result.is_err());
    }
}
