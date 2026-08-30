use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::{DeploymentInfo, DeviceType, LatLng, UpdateInfoRequest},
    keypair::Signer,
    programs::KnownProgram,
    transaction::VersionedTransaction,
    verify,
};

/// Both update instructions name the hotspot's owner at account index 3 and
/// carry the asserted location in their arguments.
const UPDATE_METHODS: &[&str] = &["update_iot_info_v0", "update_mobile_info_v0"];
const HOTSPOT_OWNER_INDEX: usize = 3;

/// Refuse to sign unless the update asserts the location that was asked for,
/// on a hotspot this wallet owns, and changes nothing that was not asked for.
///
/// Location is the field rewards are paid against, and it is compared exactly:
/// the cell is derived from the same coordinates that were sent. Gain and
/// elevation are checked only for presence, so an update that quietly adds one
/// is refused while their unit conversion stays the service's to define --
/// duplicating its rounding here would refuse honest updates the day it changes.
fn assert_updates_hotspot(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    request: &UpdateInfoRequest,
) -> Result<()> {
    let found = verify::find_methods(unsigned, KnownProgram::HeliumEntityManager, UPDATE_METHODS)?;
    let [update] = found.as_slice() else {
        bail!(
            "expected exactly one hotspot info update, found {}; refusing to sign",
            found.len()
        );
    };
    let owner = update.account(HOTSPOT_OWNER_INDEX)?;
    if owner != *wallet {
        bail!("the update is for a hotspot owned by {owner}, not this wallet");
    }

    let args = update
        .args()
        .ok_or_else(|| anyhow!("the update's arguments could not be read; refusing to sign"))?;
    let args = &args["args"];

    let (lat, lon) = match request.location {
        Some(LatLng { lat, lng }) => (Some(lat), Some(lng)),
        None => (None, None),
    };
    let want = helium_lib::hotspot::cell_for(lat, lon)?.map(u64::from);
    let got = args["location"].as_u64();
    if got != want {
        bail!("the update asserts location {got:?}, not the requested {want:?}");
    }

    for (field, requested) in [
        ("gain", request.gain.is_some()),
        ("elevation", request.elevation.is_some()),
    ] {
        if !requested && !args[field].is_null() {
            bail!("the update sets {field}, which was not asked for; refusing to sign");
        }
    }
    Ok(())
}

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
        let request = UpdateInfoRequest {
            device_type: DeviceType::Iot,
            entity_pub_key: self.gateway.to_string(),
            wallet_address: signer.pubkey().to_string(),
            location: location(self.lat, self.lon)?,
            gain: self.gain,
            elevation: self.elevation,
            azimuth: self.azimuth,
            deployment_info: None,
        };
        let response = api.update_info(&request).await?;
        assert_updates_hotspot(&response.decode_transactions()?, &signer.pubkey(), &request)?;
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
        let request = UpdateInfoRequest {
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
        };
        let response = api.update_info(&request).await?;
        assert_updates_hotspot(&response.decode_transactions()?, &signer.pubkey(), &request)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use helium_lib::{
        keypair::Pubkey,
        solana_sdk::{
            instruction::{AccountMeta, Instruction},
            message::{Message, VersionedMessage},
        },
    };

    /// The discriminator the entity-manager IDL declares for this method.
    const UPDATE_IOT: [u8; 8] = [211, 235, 205, 29, 109, 86, 153, 39];

    const LAT: f64 = 37.7749;
    const LON: f64 = -122.4194;

    fn cell(lat: f64, lon: f64) -> u64 {
        u64::from(
            helium_lib::hotspot::cell_for(Some(lat), Some(lon))
                .expect("a valid coordinate")
                .expect("a cell"),
        )
    }

    /// `UpdateIotInfoArgsV0` in IDL field order: location, elevation, gain,
    /// then the merkle proof fields the guard does not read.
    fn update_tx(
        owner: Pubkey,
        location: Option<u64>,
        elevation: Option<i32>,
        gain: Option<i32>,
    ) -> VersionedTransaction {
        let mut data = UPDATE_IOT.to_vec();
        let mut opt_u64 = |v: Option<u64>| match v {
            Some(v) => {
                data.push(1);
                data.extend_from_slice(&v.to_le_bytes());
            }
            None => data.push(0),
        };
        opt_u64(location);
        for v in [elevation, gain] {
            match v {
                Some(v) => {
                    data.push(1);
                    data.extend_from_slice(&v.to_le_bytes());
                }
                None => data.push(0),
            }
        }
        data.extend_from_slice(&[0u8; 32]); // data_hash
        data.extend_from_slice(&[0u8; 32]); // creator_hash
        data.extend_from_slice(&[0u8; 32]); // root
        data.extend_from_slice(&0u32.to_le_bytes()); // index

        let accounts = [
            Pubkey::new_unique(), // payer
            Pubkey::new_unique(), // dc_fee_payer
            Pubkey::new_unique(), // iot_info
            owner,
        ];
        let ix = Instruction {
            program_id: KnownProgram::HeliumEntityManager.id(),
            accounts: accounts
                .iter()
                .map(|key| AccountMeta::new(*key, false))
                .collect(),
            data,
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&owner))),
        }
    }

    fn request(lat: Option<f64>, lon: Option<f64>, gain: Option<f64>) -> UpdateInfoRequest {
        UpdateInfoRequest {
            device_type: DeviceType::Iot,
            entity_pub_key: "gw".to_string(),
            wallet_address: "w".to_string(),
            location: location(lat, lon).expect("a valid coordinate"),
            gain,
            elevation: None,
            azimuth: None,
            deployment_info: None,
        }
    }

    #[test]
    fn the_requested_assertion_is_accepted() {
        let wallet = Pubkey::new_unique();
        assert_updates_hotspot(
            &[update_tx(wallet, Some(cell(LAT, LON)), None, None)],
            &wallet,
            &request(Some(LAT), Some(LON), None),
        )
        .expect("the requested assertion");
    }

    #[test]
    fn an_assertion_at_another_location_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_updates_hotspot(
            &[update_tx(wallet, Some(cell(51.5074, -0.1278)), None, None)],
            &wallet,
            &request(Some(LAT), Some(LON), None),
        )
        .expect_err("a substituted location must be refused");
        assert!(err.to_string().contains("not the requested"), "{err}");
    }

    #[test]
    fn an_update_to_someone_elses_hotspot_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_updates_hotspot(
            &[update_tx(
                Pubkey::new_unique(),
                Some(cell(LAT, LON)),
                None,
                None,
            )],
            &wallet,
            &request(Some(LAT), Some(LON), None),
        )
        .expect_err("another owner's hotspot must be refused");
        assert!(err.to_string().contains("not this wallet"), "{err}");
    }

    #[test]
    fn an_update_setting_a_gain_that_was_not_asked_for_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_updates_hotspot(
            &[update_tx(wallet, Some(cell(LAT, LON)), None, Some(30))],
            &wallet,
            &request(Some(LAT), Some(LON), None),
        )
        .expect_err("an unrequested gain must be refused");
        assert!(err.to_string().contains("gain"), "{err}");
    }

    #[test]
    fn an_update_setting_a_location_that_was_not_asked_for_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_updates_hotspot(
            &[update_tx(wallet, Some(cell(LAT, LON)), None, None)],
            &wallet,
            &request(None, None, None),
        )
        .expect_err("an unrequested location must be refused");
        assert!(err.to_string().contains("not the requested"), "{err}");
    }

    #[test]
    fn a_second_update_smuggled_alongside_the_first_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_updates_hotspot(
            &[
                update_tx(wallet, Some(cell(LAT, LON)), None, None),
                update_tx(wallet, Some(cell(51.5074, -0.1278)), None, None),
            ],
            &wallet,
            &request(Some(LAT), Some(LON), None),
        )
        .expect_err("a batch updating twice must be refused");
        assert!(err.to_string().contains("found 2"), "{err}");
    }

    #[test]
    fn a_response_carrying_no_update_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_updates_hotspot(&[], &wallet, &request(Some(LAT), Some(LON), None))
            .expect_err("an action that was never built must be refused");
        assert!(err.to_string().contains("found 0"), "{err}");
    }
}
