use crate::{cmd::*, txn_envelope::TxnEnvelope};
use chrono::{DateTime, Utc};
use helium_crypto::{KeyTag, PublicKey};
use helium_lib::{
    asset,
    client::{VERIFIER_URL_DEVNET, VERIFIER_URL_MAINNET},
    dao::SubDao,
    hotspot::{self, cert, HotspotInfoUpdate},
};
use helium_proto::BlockchainTxnAddGatewayV1;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufReader, Write},
};

#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: AddCmd,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
enum AddCmd {
    Iot(Box<IotCmd>),
    Mobile(MobileCmd),
}

impl AddCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Iot(cmd) => cmd.run(opts).await,
            Self::Mobile(cmd) => cmd.run(opts).await,
        }
    }
}

/// Add an IOT Hotspot to the blockchain.
///
/// The required transaction is created by a
/// Hotspot and supplied here for owner signing.
#[derive(Clone, Debug, clap::Args)]
struct IotCmd {
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

    /// The antenna gain for the asserted IoT Hotspot in dBi, with one digit of
    /// accuracy.
    ///
    /// Defaults to the last asserted value. Note that the gain is truncated to
    /// the nearest 0.1 dBi.
    #[arg(long)]
    gain: Option<f64>,

    /// The elevation for the asserted IoT Hotspot in meters above ground level.
    ///
    /// Defaults to the last assserted value. For negative values use '=', for
    /// example: "--elevation=-xx".
    #[arg(long)]
    elevation: Option<i32>,

    /// Base64 encoded Hotspot transaction.
    txn: Transaction,

    /// Optional url for the ecc signature verifier.
    ///
    /// If the main API URL is one of the shortcuts (like "m" or "d") the
    /// default verifier for that network will be used.
    #[arg(long)]
    verifier: Option<String>,

    /// Commit the Hotspot add.
    #[command(flatten)]
    commit: CommitOpts,
}

async fn perform_add(
    subdao: SubDao,
    mut txn: BlockchainTxnAddGatewayV1,
    update: HotspotInfoUpdate,
    verifier: &Option<String>,
    commit: &CommitOpts,
    opts: &Opts,
) -> Result {
    let password = get_wallet_password(false)?;
    let keypair = opts.load_keypair(password.as_bytes())?;
    let gateway = helium_crypto::PublicKey::from_bytes(&txn.gateway)?;
    let client = opts.client()?;
    let hotspot_issued = asset::for_entity_key(&client, &gateway).await.is_ok();
    let verifier_key = verifier.as_ref().unwrap_or(&opts.url);
    let verifier = match verifier_key.as_str() {
        "m" | "mainnet-beta" => VERIFIER_URL_MAINNET,
        "d" | "devnet" => VERIFIER_URL_DEVNET,
        url => url,
    };
    let transaction_opts = &commit.transaction_opts(&client);

    if !hotspot_issued {
        let (tx, _) =
            hotspot::dataonly::issue(&client, verifier, &mut txn, &keypair, transaction_opts)
                .await?;
        let response = commit.maybe_commit(tx, &client).await?;
        print_json(&response.to_json())?;
    }
    // Only assert the Hotspot if either (a) it has already been issued before this cli
    // was run or (b) `commit` is enabled which means the previous command should have created it.
    // Without this, the command will always fail for brand new hotspots when --commit is not
    // enabled, as it cannot find the key_to_asset account or asset account.
    if hotspot_issued || commit.commit {
        let (tx, _) = hotspot::dataonly::onboard(
            &client,
            subdao,
            &gateway,
            update,
            &keypair,
            transaction_opts,
        )
        .await?;
        print_json(&commit.maybe_commit(tx, &client).await?.to_json())
    } else {
        Ok(())
    }
}

impl IotCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let txn = BlockchainTxnAddGatewayV1::from_envelope(&self.txn)?;
        let update = HotspotInfoUpdate::for_subdao(SubDao::Iot)
            .set_gain(self.gain)
            .set_elevation(self.elevation)
            .set_geo(self.lat, self.lon)?;
        perform_add(
            SubDao::Iot,
            txn,
            update,
            &self.verifier,
            &self.commit,
            &opts,
        )
        .await
    }
}

/// Add a MOBILE Hotspot to the blockchain.
///
/// The required transaction is created by using the 'txn' subcommand
#[derive(Debug, Clone, clap::Args)]
struct MobileCmd {
    #[command(subcommand)]
    cmd: MobileCommand,
}

impl MobileCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
enum MobileCommand {
    Token(MobileToken),
    Onboard(Box<MobileOnboard>),
    Cert(MobileCert),
}

impl MobileCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Token(cmd) => cmd.run(opts).await,
            Self::Onboard(cmd) => cmd.run(opts).await,
            Self::Cert(cmd) => cmd.run(opts).await,
        }
    }
}

/// Create an onboarding transaction for a mobile data-only Hotspot
#[derive(Debug, Clone, clap::Args)]
struct MobileToken {}

impl MobileToken {
    pub async fn run(&self, _opts: Opts) -> Result {
        let gw_keypair = helium_crypto::Keypair::generate(KeyTag::default(), &mut OsRng);
        let issue_token = hotspot::dataonly::issue_token(&gw_keypair)?;
        print_json(&issue_token)
    }
}

/// Onboard the given Hotspot given a transaction
///
/// Issues the mobile Hotspot NFT and onboards it given the created data-only transaction.
/// Location details are optional
#[derive(Debug, Clone, clap::Args)]
struct MobileOnboard {
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
    /// Base64 encoded add Hotspot token.
    ///
    /// The token is generated by the 'token' command
    token: Transaction,

    /// Optional url for the ecc signature verifier.
    ///
    /// If the main API URL is one of the shortcuts (like "m" or "d") the
    /// default verifier for that network will be used.
    #[arg(long)]
    verifier: Option<String>,

    /// Commit the Hotspot add.
    #[command(flatten)]
    commit: CommitOpts,
}

impl MobileOnboard {
    pub async fn run(&self, opts: Opts) -> Result {
        let txn = BlockchainTxnAddGatewayV1::from_envelope(&self.token)?;
        let update = HotspotInfoUpdate::for_subdao(SubDao::Mobile).set_geo(self.lat, self.lon)?;
        perform_add(
            SubDao::Mobile,
            txn,
            update,
            &self.verifier,
            &self.commit,
            &opts,
        )
        .await
    }
}

/// Fetches or creates the cert for a mobile only data hotspot
///
///
/// The given hotspot must be owned by the wallet requesting the cert.
/// To create a hotspot provide the location of a file with the following information:
///
///{
///    "address": "address of hotspot location",
///    "lat": 12.0587,
///    "lon": -67.08494,
///    "nas_id": "coffee_shop_el_centro"
///}
///
/// Provide the path to this file to create the certificate. For future certificate
/// requests for the given hotspots the json file is no longer needed and can be ommitted.
#[derive(Debug, Clone, clap::Args)]
struct MobileCert {
    /// The mobile hotspot to get or create the cert for
    hotspot: PublicKey,
    /// Location info file for a new cert
    ///
    /// If provided creates or gets the cert. If not provided just tries to rertieve an
    /// existing cert.
    info: Option<PathBuf>,
    /// Ouptut path prefix
    ///
    /// On success, the certification will be stored in <output>/<hotspot>.cer
    /// and the private key in <output>/<hotspot>.pk
    #[arg(long)]
    output: Option<PathBuf>,

    #[arg(long)]
    dry_run: bool,
}

fn read_info<P: AsRef<Path>>(path: P) -> Result<cert::LocationInfo> {
    let file = File::open(path)?;
    let result: MobileCertLocationInfo = serde_json::from_reader(BufReader::new(file))?;
    Ok(result.into())
}

fn write_file<P: AsRef<Path>>(path: P, txt: &str) -> Result<()> {
    let mut writer = File::create_new(path.as_ref())?;
    writer.write_all(txt.as_bytes())?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct MobileCertLocationInfo {
    pub address: String,
    pub lat: f64,
    pub lon: f64,
    pub nas_id: String,
}

#[derive(Debug, Serialize)]
pub struct MobileCertInfo {
    pub expiration: DateTime<Utc>,
    pub private_key: PathBuf,
    pub certificate: PathBuf,
}

impl From<MobileCertLocationInfo> for cert::LocationInfo {
    fn from(value: MobileCertLocationInfo) -> Self {
        Self {
            location_address: value.address,
            location_lat: value.lat,
            location_lon: value.lon,
            nas_ids: vec![value.nas_id],
        }
    }
}

impl MobileCert {
    pub async fn run(&self, opts: Opts) -> Result {
        let location_info = self.info.as_ref().map(read_info).transpose()?;
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;

        let cert_info = cert::get_or_create(
            &client,
            location_info,
            self.hotspot.clone(),
            &keypair,
            self.dry_run,
        )
        .await?;

        let base_path = self
            .output
            .to_owned()
            .unwrap_or_default()
            .as_path()
            .with_file_name(self.hotspot.to_string());

        let pk_path = base_path.as_path().with_extension("pk");
        let cert_path = base_path.as_path().with_extension("cer");
        write_file(&pk_path, &cert_info.cert.radsec_private_key)?;
        write_file(&cert_path, &cert_info.cert.radsec_certificate)?;

        let result = MobileCertInfo {
            expiration: cert_info.cert.radsec_cert_expire,
            private_key: pk_path,
            certificate: cert_path,
        };

        print_json(&result)
    }
}
