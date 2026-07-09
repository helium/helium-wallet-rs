use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::{DeploymentInfo, DeviceType, LatLng, UpdateInfoRequest},
    keypair::Signer,
};

#[derive(Debug, Clone, clap::Args)]
/// Assert a Hotspot's on-chain info (location and device details).
///
/// Only the fields you pass are changed; anything omitted keeps its current
/// on-chain value. The update is not submitted unless '--commit' is given.
pub struct Cmd {
    #[command(subcommand)]
    cmd: UpdateCmd,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum UpdateCmd {
    /// Assert an IoT Hotspot's location, gain, and elevation.
    Iot(IotCmd),
    /// Assert a Mobile Hotspot's location and WiFi deployment info.
    Mobile(MobileCmd),
}

impl UpdateCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Iot(cmd) => cmd.run(opts).await,
            Self::Mobile(cmd) => cmd.run(opts).await,
        }
    }
}

#[derive(Debug, Clone, clap::Args)]
pub struct IotCmd {
    /// Helium address of the Hotspot to assert.
    gateway: helium_crypto::PublicKey,
    /// Latitude to assert. Requires --lon. Defaults to the current value.
    #[arg(long)]
    lat: Option<f64>,
    /// Longitude to assert. Requires --lat. Defaults to the current value.
    #[arg(long)]
    lon: Option<f64>,
    /// Antenna gain in dBi (one digit of precision). Defaults to the current value.
    #[arg(long)]
    gain: Option<f64>,
    /// Elevation in meters above ground level. Defaults to the current value.
    #[arg(long)]
    elevation: Option<f64>,
    /// Antenna azimuth in degrees (0-360). Defaults to the current value.
    #[arg(long)]
    azimuth: Option<f64>,
    #[command(flatten)]
    commit: CommitOpts,
}

impl IotCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .update_info(&UpdateInfoRequest {
                device_type: DeviceType::Iot,
                entity_pub_key: self.gateway.to_string(),
                wallet_address: signer.pubkey().to_string(),
                location: location(self.lat, self.lon)?,
                gain: self.gain,
                elevation: self.elevation,
                azimuth: self.azimuth,
                deployment_info: None,
            })
            .await?;
        print_json(
            &self
                .commit
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
}

#[derive(Debug, Clone, clap::Args)]
pub struct MobileCmd {
    /// Helium address of the Hotspot to assert.
    gateway: helium_crypto::PublicKey,
    /// Latitude to assert. Requires --lon. Defaults to the current value.
    #[arg(long)]
    lat: Option<f64>,
    /// Longitude to assert. Requires --lat. Defaults to the current value.
    #[arg(long)]
    lon: Option<f64>,
    /// WiFi antenna index. Defaults to the current value.
    #[arg(long)]
    antenna: Option<i64>,
    /// Elevation in meters above ground level. Defaults to the current value.
    #[arg(long)]
    elevation: Option<f64>,
    /// Antenna azimuth in degrees (0-360). Defaults to the current value.
    #[arg(long)]
    azimuth: Option<f64>,
    /// Mechanical downtilt in degrees. Defaults to the current value.
    #[arg(long)]
    mechanical_downtilt: Option<f64>,
    /// Electrical downtilt in degrees. Defaults to the current value.
    #[arg(long)]
    electrical_downtilt: Option<f64>,
    /// Hardware serial number. Defaults to the current value.
    #[arg(long)]
    serial: Option<String>,
    #[command(flatten)]
    commit: CommitOpts,
}

impl MobileCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .update_info(&UpdateInfoRequest {
                device_type: DeviceType::Mobile,
                entity_pub_key: self.gateway.to_string(),
                wallet_address: signer.pubkey().to_string(),
                location: location(self.lat, self.lon)?,
                gain: None,
                elevation: None,
                // Mobile azimuth is carried in deployment_info (WIFI); the
                // top-level azimuth field is IoT-only.
                azimuth: None,
                deployment_info: self.deployment_info(),
            })
            .await?;
        print_json(
            &self
                .commit
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

    /// Build WiFi deployment info from the provided flags, or `None` for a
    /// location-only update when no deployment flags were given.
    fn deployment_info(&self) -> Option<DeploymentInfo> {
        let any = self.antenna.is_some()
            || self.elevation.is_some()
            || self.azimuth.is_some()
            || self.mechanical_downtilt.is_some()
            || self.electrical_downtilt.is_some()
            || self.serial.is_some();
        any.then(|| DeploymentInfo::Wifi {
            antenna: self.antenna,
            elevation: self.elevation,
            azimuth: self.azimuth,
            mechanical_down_tilt: self.mechanical_downtilt,
            electrical_down_tilt: self.electrical_downtilt,
            serial: self.serial.clone(),
        })
    }
}

/// Build a location payload from optional lat/lon. Both or neither must be
/// given; a partial pair is rejected since the API location requires both.
fn location(lat: Option<f64>, lon: Option<f64>) -> Result<Option<LatLng>> {
    match (lat, lon) {
        (Some(lat), Some(lng)) => {
            // Reject out-of-range coordinates locally so a typo fails fast
            // instead of round-tripping to the server or asserting nonsense.
            if !(-90.0..=90.0).contains(&lat) {
                bail!("latitude {lat} is out of range [-90, 90]");
            }
            if !(-180.0..=180.0).contains(&lng) {
                bail!("longitude {lng} is out of range [-180, 180]");
            }
            Ok(Some(LatLng { lat, lng }))
        }
        (None, None) => Ok(None),
        _ => bail!("both --lat and --lon are required to assert a location"),
    }
}
