use crate::cmd::*;
use helium_lib::{
    client::{ONBOARDING_URL_DEVNET, ONBOARDING_URL_MAINNET},
    dao::SubDao,
    hotspot::{self, HotspotInfoUpdate},
};

#[derive(Debug, Clone, clap::Args)]
/// Assert a Hotspot location on the blockchain.
///
/// The original transaction is
/// created by the Hotspot miner and supplied here for owner signing. Use an
/// onboarding key to get the transaction signed by the DeWi staking server.
pub struct Cmd {
    /// The subdao to assert the Hotspot on
    subdao: SubDao,

    /// Helium address of Hotspot to assert
    gateway: helium_crypto::PublicKey,

    /// Latitude of Hotspot location to assert.
    ///
    /// Defaults to the last asserted value. For negative values use '=', for
    /// example: "--lat=-xx.xxxxxxx".
    #[arg(long)]
    lat: Option<f64>,

    /// Longitude of Hotspot location to assert.
    ///
    /// Defaults to the last asserted value. For negative values use '=', for
    /// example: "--lon=-xx.xxxxxxx".
    #[arg(long)]
    lon: Option<f64>,

    /// The antenna gain for the asserted Hotspot in dBi, with one digit of
    /// accuracy.
    ///
    /// Defaults to the last asserted value. Note that the gain is truncated to
    /// the nearest 0.1 dBi.
    #[arg(long)]
    gain: Option<f64>,

    /// The elevation for the asserted Hotspot in meters above ground level.
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

        let server = self.onboarding.as_ref().map(|value| {
            match value.as_str() {
                "m" | "mainnet-beta" => ONBOARDING_URL_MAINNET,
                "d" | "devnet" => ONBOARDING_URL_DEVNET,
                url => url,
            }
            .to_string()
        });

        let update = HotspotInfoUpdate::for_subdao(self.subdao)
            .set_gain(self.gain)
            .set_elevation(self.elevation)
            .set_geo(self.lat, self.lon)?;

        let client = opts.client()?;
        let transaction_opts = self.commit.transaction_opts(&client);
        let tx = hotspot::update(
            &client,
            server,
            &self.gateway,
            update,
            &keypair,
            &transaction_opts,
        )
        .await?;

        print_json(&self.commit.maybe_commit(tx, &client).await.to_json())
    }
}
