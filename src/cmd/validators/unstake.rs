use crate::{
    cmd::*,
    keypair::Keypair,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign},
};
use serde::Deserialize;

#[derive(Debug, StructOpt)]
/// Unstake a given validator. The stake will be in a cooldown period after
/// unstaking before the HNT is returned to the owning wallet.
///
/// The command requires a 'stake-release-height' argument which is suggested to
/// be at least the current block height plus the chain cooldown period (as
/// defined by a chain variable), and 5-10 blocks to allow for chain
/// processing delays.
///
/// Note that multiple staking transactions are submitted individually and not as a
/// single transaction. Any failures will abort the remaining staking entries.
pub enum Cmd {
    /// Unstake a single validator
    One(Box<One>),
    /// Unstake multiple validators via file import
    Multi(Box<Multi>),
}

#[derive(Debug, StructOpt)]
pub struct One {
    #[structopt(flatten)]
    validator: UnstakeValidator,
    #[structopt(long)]
    fee: Option<u64>,
    /// The stake release block height.
    #[structopt(long)]
    stake_release_height: Option<u64>,
    /// Whether to commit the transaction(s) to the blockchain
    #[structopt(long)]
    commit: bool,
}

#[derive(Debug, StructOpt)]
/// The input file for multiple validator is expected to be json file
/// with a list of address, stake_release_height and stake (the original stake of the validator). For example:
///
/// [
///     {
///         "address": "<adddress1>",
///         "stake_release_height": 1440223,
///         "stake": 10000
///     },
///     {
///         "address": "<adddress2>",
///         "stake": 10000
///     }
/// ]
///
/// If stake_release_height is not specified for a validator in this array, it will be taken from the command argument with the same name.
pub struct Multi {
    /// File to read multiple stakes from
    path: PathBuf,
    /// Manually set fee to pay for the transaction(s)
    #[structopt(long)]
    fee: Option<u64>,
    /// The stake release block height.
    #[structopt(long)]
    stake_release_height: Option<u64>,
    /// Whether to commit the transaction(s) to the blockchain
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let validators = self.collect_unstake_validators()?;

        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let client = new_client(api_url(wallet.public_key.network));
        let fee_config = if self.fee().is_none() {
            Some(get_txn_fees(&client).await?)
        } else {
            None
        };

        for validator in validators {
            if validator.address.network != wallet.public_key.network {
                bail!(
                    "valiator: {} is not on {}",
                    validator.address.to_string(),
                    wallet.public_key.network
                )
            }
            let txn = self
                .mk_txn(&client, &keypair, &fee_config, &validator)
                .await?;
            let envelope = txn.in_envelope();
            let status = maybe_submit_txn(self.commit(), &client, &envelope).await?;
            print_txn(&envelope, &txn, &status, &opts.format)?
        }

        Ok(())
    }

    async fn mk_txn(
        &self,
        client: &Client,
        keypair: &Keypair,
        fee_config: &Option<TxnFeeConfig>,
        validator: &UnstakeValidator,
    ) -> Result<BlockchainTxnUnstakeValidatorV1> {
        let mut txn = BlockchainTxnUnstakeValidatorV1 {
            address: validator.address.to_vec(),
            owner: keypair.public_key().to_vec(),
            stake_amount: if let Some(stake_amount) = validator.stake {
                u64::from(stake_amount)
            } else {
                u64::from(
                    helium_api::validators::get(client, &validator.address.to_string())
                        .await?
                        .stake,
                )
            },
            fee: 0,
            owner_signature: vec![],
            stake_release_height: validator.stake_release_height.unwrap(),
        };
        txn.fee = if let Some(fee) = self.fee() {
            fee
        } else {
            txn.txn_fee(fee_config.as_ref().unwrap())?
        };

        txn.owner_signature = txn.sign(keypair)?;
        Ok(txn)
    }

    fn fee(&self) -> Option<u64> {
        match &self {
            Self::One(one) => one.fee,
            Self::Multi(multi) => multi.fee,
        }
    }

    fn collect_unstake_validators(&self) -> Result<Vec<UnstakeValidator>> {
        match &self {
            Self::One(one) => {
                Ok(vec![self.mk_unstake_validator(
                    &one.validator,
                    one.stake_release_height,
                )?])
            }
            Self::Multi(multi) => {
                let file = std::fs::File::open(multi.path.clone())?;
                let validators: Vec<UnstakeValidator> = serde_json::from_reader(file)?;
                let validators = validators
                    .iter()
                    .map(|v| self.mk_unstake_validator(v, multi.stake_release_height))
                    .collect::<Result<Vec<UnstakeValidator>>>()?;
                Ok(validators)
            }
        }
    }

    fn mk_unstake_validator(
        &self,
        validator: &UnstakeValidator,
        stake_release_height: Option<u64>,
    ) -> Result<UnstakeValidator> {
        let mut validator = validator.clone();
        validator.stake_release_height = match validator.stake_release_height {
            Some(h) => Some(h),
            None => match stake_release_height {
                Some(h) => Some(h),
                None => bail!("stake-relase-height must be specified, either for each validator in the validators file, or in the command."),
            }
        };
        Ok(validator)
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
    txn: &BlockchainTxnUnstakeValidatorV1,
    status: &Option<PendingTxnStatus>,
    format: &OutputFormat,
) -> Result {
    let validator = PublicKey::from_bytes(&txn.address)?.to_string();
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Validator", validator],
                ["Fee (DC)", txn.fee],
                ["Stake (HNT)", Hnt::from(txn.stake_amount)],
                ["Hash", status_str(status)],
                [Frb => "WARNING",
                "After unstaking, a validator can not access the staked amount\n\
                nor earn rewards for 250,000 blocks (approx. five months)."]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "validator" : validator,
                "fee": txn.fee,
                "stake_amount": txn.stake_amount,
                "txn": envelope.to_b64()?,
                "hash": status_json(status)
            });
            print_json(&table)
        }
    }
}

#[derive(Debug, Deserialize, StructOpt, Clone)]
pub struct UnstakeValidator {
    // The validator address to unstake
    address: PublicKey,
    /// The amount of HNT of the original stake
    #[structopt(long)]
    stake: Option<Hnt>,
    /// The stake release block height. This should be at least the current
    /// block height plus the cooldown period, and 5-10 blocks to allow for
    /// chain processing delays.
    /// This field is optional, and stake release height from the command arguments will be used if this is not suplied.
    #[structopt(skip)]
    stake_release_height: Option<u64>,
}
