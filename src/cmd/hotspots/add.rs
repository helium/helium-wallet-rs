use crate::{
    client::HotspotAssertion, cmd::*, dao::SubDao, hotspot::HotspotMode, result::Result,
    traits::txn_envelope::TxnEnvelope,
};
use helium_proto::BlockchainTxnAddGatewayV1;

#[derive(Clone, Debug, clap::Args)]
/// Add a hotspot to the blockchain. The original transaction is created by the
/// hotspot miner and supplied here for owner signing. Use an onboarding key to
/// get the transaction signed by the DeWi staking server.
pub struct Cmd {
    /// The subdao to assert the hotspot on. Only iot is currently supported.
    #[arg(long)]
    subdao: SubDao,

    /// The mode of the hotspot to add. Only "dataonly" is currently supported.
    #[arg(long)]
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
    #[arg(long, default_value = "https://ecc-verifier.web.helium.io")]
    verifier_url: String,

    /// Commit the hotspot add.
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        if self.subdao != SubDao::Iot {
            bail!("Only iot subdao is currently supported")
        }
        if self.mode != HotspotMode::DataOnly {
            bail!("Only dataonly mode is currently supported")
        }

        let mut txn = BlockchainTxnAddGatewayV1::from_envelope(&read_txn(&self.txn)?)?;
        let password = get_wallet_password(false)?;
        let wallet = load_wallet(&opts.files)?;
        let client = new_client(&opts.url)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        if client.hotspot_key_to_asset(&txn.gateway).is_err() {
            let tx =
                client.hotspot_dataonly_issue(&self.verifier_url, &mut txn, keypair.clone())?;
            self.commit.maybe_commit(&tx, &client)?;
        }
        let assertion =
            HotspotAssertion::try_from((self.lat, self.lon, self.elevation, self.gain))?;
        let tx = client.hotspot_dataonly_onboard(&txn.gateway, assertion, keypair)?;
        self.commit.maybe_commit(&tx, &client)
    }
}
