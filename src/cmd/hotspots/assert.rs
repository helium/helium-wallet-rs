use crate::{
    cmd::*,
    staking,
    traits::{TxnEnvelope, TxnFee, TxnSign, TxnStakingFee},
};
use helium_api::{hotspots, Dbi};

#[derive(Debug, StructOpt)]
/// Assert a hotspot location on the blockchain. The original transaction is
/// created by the hotspot miner and supplied here for owner signing. Use an
/// onboarding key to get the transaction signed by the DeWi staking server.
pub struct Cmd {
    /// Address of hotspot to assert
    #[structopt(long)]
    gateway: PublicKey,

    /// Lattitude of hotspot location to assert
    #[structopt(long)]
    lat: f64,

    /// Longitude of hotspot location to assert
    #[structopt(long)]
    lon: f64,

    /// The antenna gain for the asserted hotspotin dBi, with one digit of
    /// accuracy. Defaults to the last asserted value.
    #[structopt(long)]
    gain: Option<Dbi>,

    /// The elevation for the asserted hotspot in meters relative to sea level.
    /// Defaults to the last assserted value
    #[structopt(long)]
    elevation: Option<i32>,

    /// Use the DeWi "staking" server to pay for the assert location. Note that
    /// no, or only a limited number of asserts may available for use by the
    /// staking server.
    #[structopt(long)]
    onboarding: bool,

    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let staking_client = staking::Client::default();
        let client = helium_api::Client::new_with_base_url(api_url(wallet.public_key.network));
        let hotspot = hotspots::get(&client, &self.gateway.to_string()).await?;
        let gain: i32 = if let Some(gain) = self.gain.or(hotspot.gain) {
            gain.into()
        } else {
            bail!("no gain specified or found on chain")
        };
        let elevation = if let Some(elevation) = self.elevation.or(hotspot.elevation) {
            elevation
        } else {
            bail!("no elevation specified or found on chain")
        };

        let wallet_key = keypair.public_key();
        // Get the next likely gateway nonce for the new transaction
        let nonce = helium_api::hotspots::get(&client, &self.gateway.to_string())
            .await?
            .speculative_nonce
            + 1;
        let payer = if self.onboarding {
            staking_client.address_for(&self.gateway).await?.into()
        } else {
            wallet.public_key.into()
        };
        let location: geo_types::Point<f64> = (self.lon, self.lat).into();
        let mut txn = BlockchainTxnAssertLocationV2 {
            payer,
            owner: wallet_key.into(),
            gateway: self.gateway.clone().into(),
            location: h3ron::H3Cell::from_point(&location, 12)?.to_string(),
            elevation,
            gain,
            nonce,
            owner_signature: vec![],
            payer_signature: vec![],
            staking_fee: 0,
            fee: 0,
        };

        let fees = &get_txn_fees(&client).await?;
        txn.fee = txn.txn_fee(&fees)?;
        txn.staking_fee = txn.txn_staking_fee(&fees)?;

        txn.owner_signature = txn.sign(&keypair)?;

        let envelope = if self.onboarding {
            staking_client
                .sign(&self.gateway.to_string(), &txn.in_envelope())
                .await
        } else {
            txn.payer_signature = txn.owner_signature.clone();
            Ok(txn.in_envelope())
        }?;

        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_txn(&txn, &envelope, &status, opts.format)
    }
}

fn print_txn(
    txn: &BlockchainTxnAssertLocationV2,
    envelope: &BlockchainTxn,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let address = PublicKey::from_bytes(&txn.gateway)?.to_string();
    let payer = if txn.payer.is_empty() {
        PublicKey::from_bytes(&txn.owner)?.to_string()
    } else {
        PublicKey::from_bytes(&txn.payer)?.to_string()
    };
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Address", address],
                ["Location", txn.location],
                ["Payer", payer],
                ["Nonce", txn.nonce],
                ["Fee (DC)", txn.fee],
                ["Staking Fee (HNT)", Hnt::from(txn.staking_fee)],
                ["Gain (dBi)", Dbi::from(txn.gain)],
                ["Elevation", txn.elevation],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let table = json!({
                "address": address,
                "location": txn.location,
                "gain": txn.gain,
                "elevation": txn.elevation,
                "payer": payer,
                "fee": txn.fee,
                "nonce": txn.nonce,
                "staking_fee": txn.staking_fee,
                "hash": status_json(status),
                "txn": envelope.to_b64()?,
            });
            print_json(&table)
        }
    }
}
