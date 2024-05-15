use crate::{cmd::*, txn_envelope::TxnEnvelope};
use helium_lib::{
    asset,
    dao::SubDao,
    hotspot::{self, HotspotInfoUpdate, HotspotMode},
    settings::{VERIFIER_URL_DEVNET, VERIFIER_URL_MAINNET},
};
use helium_proto::BlockchainTxnAddGatewayV1;

#[derive(Clone, Debug, clap::Args)]
/// Add a hotspot to the blockchain.
///
/// The original transaction is created by the
/// hotspot miner and supplied here for owner signing. Use an onboarding key to
/// get the transaction signed by the DeWi staking server.
pub struct Cmd {
    /// The subdao to assert the hotspot on. Only "iot" is currently supported.
    subdao: SubDao,

    /// The mode of the hotspot to add. Only "data-only" is currently supported.
    #[arg(long, default_value = "data-only")]
    mode: HotspotMode,

    /// Lattitude of hotspot location to assert.
    ///
    /// Defaults to the last asserted value. For negative values use '=', for
    /// example: "--lat=-xx.xxxxxxx".
    #[arg(long)]
    lat: Option<f64>,

    /// Longitude of hotspot location to assert.
    ///
    /// Defaults to the last asserted value. For negative values use '=', for
    /// example: "--lon=-xx.xxxxxxx".
    #[arg(long)]
    lon: Option<f64>,

    /// The antenna gain for the asserted hotspotin dBi, with one digit of
    /// accuracy.
    ///
    /// Defaults to the last asserted value. Note that the gain is truncated to
    /// the nearest 0.1 dBi.
    #[arg(long)]
    gain: Option<f64>,

    /// The elevation for the asserted hotspot in meters above ground level.
    ///
    /// Defaults to the last assserted value. For negative values use '=', for
    /// example: "--elevation=-xx".
    #[arg(long)]
    elevation: Option<i32>,

    /// Base64 encoded transaction. If no transaction is given stdin is read for
    /// the transaction.
    ///
    ///  Note that the stdin feature only works if the wallet password is set in
    /// the HELIUM_WALLET_PASSWORD environment variable
    txn: Option<Transaction>,

    /// Optional url for the ecc signature verifier.
    ///
    /// If the main API URL is one of the shortcuts (like "m" or "d") the
    /// default verifier for that network will be used.
    #[arg(long)]
    verifier: Option<String>,

    /// Commit the hotspot add.
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        if self.subdao != SubDao::Iot {
            bail!("Only iot subdao is currently supported")
        }
        if self.mode != HotspotMode::DataOnly {
            bail!("Only dataonly mode is currently supported")
        }

        let mut txn = BlockchainTxnAddGatewayV1::from_envelope(&read_txn(&self.txn)?)?;
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let hotspot_key = helium_crypto::PublicKey::from_bytes(&txn.gateway)?;
        let verifier_key = self.verifier.as_ref().unwrap_or(&opts.url);
        let settings = opts.clone().try_into()?;
        let hotspot_issued = asset::for_entity_key(&settings, &hotspot_key).await.is_ok();
        let verifier = match verifier_key.as_str() {
            "m" | "mainnet-beta" => VERIFIER_URL_MAINNET,
            "d" | "devnet" => VERIFIER_URL_DEVNET,
            url => url,
        };

        if !hotspot_issued {
            let tx =
                hotspot::dataonly::issue(&settings, verifier, &mut txn, keypair.clone()).await?;
            let _ = self.commit.maybe_commit(&tx, &settings).await?;
        }
        // Only assert the hotspot if either (a) it has already been issued before this cli was run or (b) `commit` is enabled,
        // which means the previous command should have created it.
        // Without this, the command will always fail for brand new hotspots when --commit is not enabled, as it cannot find
        // the key_to_asset account or asset account.
        if hotspot_issued || self.commit.commit {
            let update = HotspotInfoUpdate::for_subdao(self.subdao)
                .set_gain(self.gain)
                .set_elevation(self.elevation)
                .set_geo(self.lat, self.lon)?;
            let tx = hotspot::dataonly::onboard(&settings, &hotspot_key, update, keypair).await?;
            print_json(&self.commit.maybe_commit(&tx, &settings).await?.to_json())
        } else {
            Ok(())
        }
    }
}
