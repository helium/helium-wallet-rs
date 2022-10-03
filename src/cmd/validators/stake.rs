use crate::{
    cmd::*,
    keypair::Keypair,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign},
};
use serde::Deserialize;

#[derive(Debug, StructOpt)]
/// Onboard one (or more) validators  with this wallet.
///
/// The payment is not submitted to the system unless the '--commit' option is
/// given.
///
/// Note that multiple staking transactions are submitted individually and not as a
/// single transaction. Any failures will abort the remaining staking entries.
pub enum Cmd {
    /// Stake a single validator
    One(One),
    /// Stake multiple validators via file import
    Multi(Multi),
}

#[derive(Debug, StructOpt)]
pub struct One {
    #[structopt(flatten)]
    validator: StakeValidator,
    /// Manually set fee to pay for the transaction(s)
    #[structopt(long)]
    fee: Option<u64>,
    /// Whether to commit the transaction(s) to the blockchain
    #[structopt(long)]
    commit: bool,
}

#[derive(Debug, StructOpt)]
/// The input file for multiple validator stakes is expected to be json file
/// with a list of address and staking amounts. For example:
///
/// [
///     {
///         "address": "<adddress1>",
///         "stake": 10000
///     },
///     {
///         "address": "<adddress2>",
///         "stake": 10000
///     }
/// ]
pub struct Multi {
    /// File to read multiple stakes from
    path: PathBuf,
    /// Manually set fee to pay for the transaction(s)
    #[structopt(long)]
    fee: Option<u64>,
    /// Whether to commit the transaction(s) to the blockchain
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let validators = self.collect_stake_validators()?;
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;
        let base_url: String = api_url(wallet.public_key.network);
        let client = new_client(base_url.clone());
        let pending_url = base_url + "/pending_transactions/";
        let fee_config = if self.fee().is_none() {
            Some(get_txn_fees(&client).await?)
        } else {
            None
        };

        for validator in validators {
            if validator.address.network != wallet.public_key.network {
                bail!(
                    "validator: {} is not on {}",
                    validator.address.to_string(),
                    wallet.public_key.network
                )
            }
            let txn = self.mk_txn(&keypair, &fee_config, &validator)?;
            let envelope = txn.in_envelope();
            let status = maybe_submit_txn(self.commit(), &client, &envelope).await?;
            print_txn(&envelope, &txn, &status, &pending_url, &opts.format)?
        }
        Ok(())
    }

    fn mk_txn(
        &self,
        keypair: &Keypair,
        fee_config: &Option<TxnFeeConfig>,
        validator: &StakeValidator,
    ) -> Result<BlockchainTxnStakeValidatorV1> {
        let mut txn = BlockchainTxnStakeValidatorV1 {
            address: validator.address.to_vec(),
            owner: keypair.public_key().to_vec(),
            stake: u64::from(validator.stake),
            fee: 0,
            owner_signature: vec![],
        };
        txn.fee = if let Some(fee) = self.fee() {
            fee
        } else {
            txn.txn_fee(fee_config.as_ref().unwrap())?
        };
        txn.owner_signature = txn.sign(keypair)?;
        Ok(txn)
    }

    fn collect_stake_validators(&self) -> Result<Vec<StakeValidator>> {
        match &self {
            Self::One(one) => Ok(vec![one.validator.clone()]),
            Self::Multi(multi) => {
                let file = std::fs::File::open(multi.path.clone())?;
                let validators: Vec<StakeValidator> = serde_json::from_reader(file)?;
                Ok(validators)
            }
        }
    }

    fn fee(&self) -> Option<u64> {
        match &self {
            Self::One(one) => one.fee,
            Self::Multi(multi) => multi.fee,
        }
    }

    fn commit(&self) -> bool {
        match &self {
            Self::One(one) => one.commit,
            Self::Multi(multi) => multi.commit,
        }
    }
}

fn print_txn(
    envelope: &BlockchainTxn,
    txn: &BlockchainTxnStakeValidatorV1,
    status: &Option<PendingTxnStatus>,
    pending_url: &str,
    format: &OutputFormat,
) -> Result {
    let validator = PublicKey::from_bytes(&txn.address)?.to_string();
    let status_endpoint = pending_url.to_owned() + status_str(status);
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Validator", validator],
                ["Stake (HNT)", Hnt::from(txn.stake)],
                ["Fee (DC)", txn.fee],
                ["Hash", status_str(status)],
                ["Status", status_endpoint],
                [Frb => "WARNING",
                "Once staked an owner cannot access the staked amount until\n\
                125,000 blocks (approx. 2.5 months) after unstaking."]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "validator" : validator,
                "fee": txn.fee,
                "stake": Hnt::from(txn.stake).to_f64(),
                "txn": envelope.to_b64()?,
                "hash": status_json(status),
                "status": status_endpoint
            });
            print_json(&table)
        }
    }
}

#[derive(Debug, Deserialize, StructOpt, Clone)]
pub struct StakeValidator {
    /// The validator address to stake
    address: PublicKey,
    /// The amount of HNT to stake
    #[serde(deserialize_with = "hnt_decimal_deserializer")]
    stake: Hnt,
}

// By default, helium-api-rs serializes and deserializes tokens as u64s (ie: bones).
// This overrides the default serializer when parsing JSON.
// Structopt deserializes using "from_str", which also expects Decimal representation
fn hnt_decimal_deserializer<'de, D>(deserializer: D) -> std::result::Result<Hnt, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let decimal: rust_decimal::Decimal = Deserialize::deserialize(deserializer)?;
    Ok(Hnt::new(decimal))
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_json_validator_stake() {
        let json_stake_input = "{\
            \"address\": \"13buBykFQf5VaQtv7mWj2PBY9Lq4i1DeXhg7C4Vbu3ppzqqNkTH\",\
            \"stake\": 10000\
        }";

        let stake: StakeValidator = serde_json::from_str(json_stake_input).unwrap();
        assert_eq!(Hnt::new(Decimal::new(10000, 0)), stake.stake);
    }
}
