use crate::cmd::*;
use helium_lib::{
    dao::SubDao,
    hotspot,
    settings::{ONBOARDING_URL_DEVNET, ONBOARDING_URL_MAINNET},
};

#[derive(Debug, Clone, clap::Args)]
/// Assert a hotspot location on the blockchain.
///
/// The original transaction is
/// created by the hotspot miner and supplied here for owner signing. Use an
/// onboarding key to get the transaction signed by the DeWi staking server.
pub struct Cmd {
    /// The subdao to assert the hotspot on
    subdao: SubDao,

    /// Helium address of hotspot to assert
    gateway: helium_crypto::PublicKey,

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

    /// The onboarding server to use for asserting the hotspot.
    ///
    /// If the API URL is specified with a shortcut like "m" or "d", the
    /// default onboarding server for that network will be used.
    #[arg(long)]
    onboarding: Option<String>,

    /// Commit the assertion.
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;

        let settings = opts.clone().try_into()?;
        let server_key = self.onboarding.as_ref().unwrap_or(&opts.url);
        let server = match server_key.as_str() {
            "m" | "mainnet-beta" => ONBOARDING_URL_MAINNET,
            "d" | "devnet" => ONBOARDING_URL_DEVNET,
            url => url,
        };

        let assertion =
            hotspot::HotspotAssertion::try_from((self.lat, self.lon, self.elevation, self.gain))?;
        let tx = hotspot::assert(server, self.subdao, &self.gateway, assertion, keypair).await?;

        print_json(&self.commit.maybe_commit(&tx, &settings).await?.to_json())
    }
}