use crate::{cmd::*, result::Context, txn_envelope::TxnEnvelope};
use chrono::{DateTime, Utc};
use helium_crypto::{KeyTag, Keypair, PublicKey, Sign};
use helium_lib::{
    asset, b64,
    blockchain_api::types::{
        DataOnlyNetwork, IssueDataOnlyHotspotRequest, OnboardDataOnlyHotspotRequest,
    },
    hotspot::{self, cert},
    keypair::Signer,
    verify,
};
use helium_proto::{BlockchainTxn, BlockchainTxnAddGatewayV1, Message, Txn};
use rand::rngs::OsRng;
use serde::Serialize;
use std::{io::Write, time::Duration};

const ISSUED_ASSET_VISIBILITY_TIMEOUT: Duration = Duration::from_secs(60);
const ISSUED_ASSET_POLL_INTERVAL: Duration = Duration::from_secs(2);

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
/// The required transaction is created by a Hotspot and supplied here for owner
/// signing. The issue and onboard transactions are built by the blockchain-api
/// (which co-signs issue with the ECC verifier) and signed locally.
#[derive(Clone, Debug, clap::Args)]
struct IotCmd {
    /// Latitude of Hotspot location to assert.
    ///
    /// For negative values use '=', for example: "--lat=-xx.xxxxxxx".
    #[arg(long)]
    lat: Option<f64>,

    /// Longitude of Hotspot location to assert.
    ///
    /// For negative values use '=', for example: "--lon=-xx.xxxxxxx".
    #[arg(long)]
    lon: Option<f64>,

    /// The antenna gain for the asserted IoT Hotspot in dBi, with one digit of
    /// accuracy. Truncated to the nearest 0.1 dBi.
    #[arg(long)]
    gain: Option<f64>,

    /// The elevation for the asserted IoT Hotspot in meters above ground level.
    ///
    /// For negative values use '=', for example: "--elevation=-xx".
    #[arg(long)]
    elevation: Option<i32>,

    /// Base64 encoded Hotspot transaction.
    txn: Transaction,

    /// Commit the Hotspot add.
    #[command(flatten)]
    commit: CommitOpts,
}

/// Location and antenna details to assert when onboarding. `gain`/`elevation`
/// apply to IoT only.
struct AddAssertion {
    lat: Option<f64>,
    lon: Option<f64>,
    gain: Option<f64>,
    elevation: Option<i32>,
}

async fn perform_add(
    network: DataOnlyNetwork,
    add_txn: BlockchainTxnAddGatewayV1,
    assertion: AddAssertion,
    commit: &CommitOpts,
    opts: &Opts,
) -> Result {
    let signer = opts.load_signer()?;
    let client = opts.client()?;
    let api = opts.blockchain_api()?;
    let wallet = signer.pubkey().to_string();
    let gateway = helium_crypto::PublicKey::from_bytes(&add_txn.gateway)?;

    // Re-encode the add-gateway envelope so the issue endpoint can co-sign it
    // with the ECC verifier.
    let add_gateway_txn = b64::encode_message(&BlockchainTxn {
        txn: Some(Txn::AddGateway(add_txn)),
    })?;

    // Skip issuing if the hotspot NFT already exists (idempotent re-runs).
    let hotspot_issued = asset::for_entity_key(&client, &gateway).await.is_ok();

    let issue_response = if hotspot_issued {
        CommitResponse::None
    } else {
        let response = api
            .issue_data_only_hotspot(&IssueDataOnlyHotspotRequest {
                wallet_address: wallet.clone(),
                add_gateway_txn,
            })
            .await?;
        verify::assert_hotspot_issue(&response.decode_transactions()?, &signer.pubkey())?;
        // The onboarding server co-signs issue with the ECC verifier; preserve
        // that signature and the server's blockhash when adding the owner's.
        commit
            .commit_via_api(
                &api,
                &client,
                &response,
                &*signer,
                ApiSigning::PreserveCosigned,
            )
            .await
            .context("while issuing the hotspot")?
    };

    // The DAS indexer lags on-chain confirmation by a few seconds; wait for the
    // freshly-issued asset to become visible before onboarding, which the
    // onboarding server resolves through DAS. Any committed issue (single tx or
    // batch) needs the wait.
    if issue_response.committed() {
        asset::wait_for_entity_key(
            &client,
            &gateway,
            ISSUED_ASSET_VISIBILITY_TIMEOUT,
            ISSUED_ASSET_POLL_INTERVAL,
        )
        .await?;
    }

    // Onboard only once the hotspot exists on-chain: either it was already
    // issued, or `--commit` submitted the issue above. Without this, a dry run
    // of a brand-new hotspot has nothing to onboard.
    let onboard_response = if hotspot_issued || commit.commit {
        let response = api
            .onboard_data_only_hotspot(&OnboardDataOnlyHotspotRequest {
                wallet_address: wallet,
                network,
                hotspot_address: gateway.to_string(),
                lat: assertion.lat,
                lng: assertion.lon,
                elevation: assertion.elevation,
                gain: assertion.gain,
            })
            .await?;
        verify::assert_hotspot_onboard(
            &response.decode_transactions()?,
            &signer.pubkey(),
            assertion.lat,
            assertion.lon,
        )?;
        commit
            .commit_via_api(
                &api,
                &client,
                &response,
                &*signer,
                ApiSigning::FreshBlockhash,
            )
            .await
            .context("while onboarding the hotspot")?
    } else {
        CommitResponse::None
    };

    print_json(&json!({
        "issue": issue_response.to_json(),
        "onboard": onboard_response.to_json(),
    }))
}

impl IotCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let txn = BlockchainTxnAddGatewayV1::from_envelope(&self.txn)?;
        perform_add(
            DataOnlyNetwork::Iot,
            txn,
            AddAssertion {
                lat: self.lat,
                lon: self.lon,
                gain: self.gain,
                elevation: self.elevation,
            },
            &self.commit,
            &opts,
        )
        .await
    }
}

/// Add a MOBILE Hotspot to the blockchain.
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

/// A newly generated data-only hotspot key with its animal name.
#[derive(Debug, Serialize)]
struct IssueHotspot {
    key: PublicKey,
    name: String,
}

/// A base64-encoded add-gateway token paired with the hotspot it was generated
/// for.
#[derive(Debug, Serialize)]
struct IssueToken {
    hotspot: IssueHotspot,
    token: String,
}

/// Generate a fresh gateway keypair and a signed add-gateway token for it. This
/// is offline: no chain interaction, just key generation and a helium-key
/// signature over the add-gateway envelope.
fn issue_token(gw_keypair: &Keypair) -> Result<IssueToken> {
    let mut txn = BlockchainTxnAddGatewayV1 {
        gateway: gw_keypair.public_key().to_vec(),
        gateway_signature: vec![],
        owner: vec![],
        owner_signature: vec![],
        payer: vec![],
        payer_signature: vec![],
        fee: 0,
        staking_fee: 0,
    };
    txn.gateway_signature = gw_keypair.sign(&txn.encode_to_vec())?;

    let envelope = BlockchainTxn {
        txn: Some(Txn::AddGateway(txn)),
    };
    Ok(IssueToken {
        hotspot: IssueHotspot {
            key: gw_keypair.public_key().clone(),
            name: hotspot::name(gw_keypair.public_key()),
        },
        token: b64::encode_message(&envelope)?,
    })
}

/// Create an add-gateway token for a mobile data-only Hotspot
#[derive(Debug, Clone, clap::Args)]
struct MobileToken {}

impl MobileToken {
    pub async fn run(&self, _opts: Opts) -> Result {
        let gw_keypair = Keypair::generate(KeyTag::default(), &mut OsRng);
        print_json(&issue_token(&gw_keypair)?)
    }
}

/// Onboard the given mobile data-only Hotspot.
///
/// Issues the mobile Hotspot NFT and onboards it given the add-gateway token
/// (from the `token` command) and location details.
#[derive(Debug, Clone, clap::Args)]
struct MobileOnboard {
    /// Latitude of Hotspot location to assert.
    ///
    /// For negative values use '=', for example: "--lat=-xx.xxxxxxx".
    #[arg(long)]
    lat: f64,
    /// Longitude of Hotspot location to assert.
    ///
    /// For negative values use '=', for example: "--lon=-xx.xxxxxxx".
    #[arg(long)]
    lon: f64,
    /// Base64 encoded add Hotspot token (from the 'token' command).
    token: Transaction,
    /// Commit the Hotspot add.
    #[command(flatten)]
    commit: CommitOpts,
}

impl MobileOnboard {
    pub async fn run(&self, opts: Opts) -> Result {
        let txn = BlockchainTxnAddGatewayV1::from_envelope(&self.token)?;
        perform_add(
            DataOnlyNetwork::Mobile,
            txn,
            AddAssertion {
                lat: Some(self.lat),
                lon: Some(self.lon),
                gain: None,
                elevation: None,
            },
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
/// To create a hotspot provide the address and nas ID of the hotspot
///
///  For future certificate requests for the given hotspots nas ID and address are
/// not needed.
#[derive(Debug, Clone, clap::Args)]
struct MobileCert {
    /// The mobile hotspot to get or create the cert for
    hotspot: PublicKey,
    /// Address of the hotspot for a newly created cert
    #[arg(long)]
    address: Option<String>,
    /// NAS ID for a newly created cert
    #[arg(long)]
    nas_id: Option<String>,
    /// Overwrite existing files
    #[arg(long)]
    force: bool,
    /// Output path prefix
    ///
    /// On success, the certification will be stored in <output>/<hotspot>.cer
    /// and the private key in <output>/<hotspot>.pk
    #[arg(long)]
    output: Option<PathBuf>,
}

fn write_file<P: AsRef<Path>>(path: P, txt: &str, create: bool) -> Result<()> {
    let mut writer = open_output_file(path.as_ref(), create).context(format!(
        "while opening {} for output",
        path.as_ref().to_str().unwrap_or("file")
    ))?;
    writer.write_all(txt.as_bytes())?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct MobileCertInfo {
    pub expiration: DateTime<Utc>,
    pub private_key: PathBuf,
    pub certificate: PathBuf,
    pub ca_chain: PathBuf,
}

impl MobileCert {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_wallet_password(false)?;
        let keypair = opts.load_keypair(password.as_bytes())?;
        let client = opts.client()?;

        let location_info = match (&self.address, &self.nas_id) {
            (Some(address), Some(nas_id)) => Some(cert::LocationInfo {
                location_address: address.to_owned(),
                nas_ids: vec![nas_id.clone()],
            }),
            (None, None) => None,
            (_, _) => bail!("both address and nas-id must be provided"),
        };

        let cert_info = cert::get_or_create(
            &client,
            location_info,
            self.hotspot.clone(),
            &keypair,
            false,
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
        let ca_path = base_path
            .as_path()
            .with_file_name("data-only")
            .with_extension("ca");

        write_file(&pk_path, &cert_info.cert.radsec_private_key, !self.force)?;
        write_file(&cert_path, &cert_info.cert.radsec_certificate, !self.force)?;
        write_file(&ca_path, &cert_info.cert.radsec_ca_chain, !self.force)?;

        let result = MobileCertInfo {
            expiration: cert_info.cert.radsec_cert_expire,
            private_key: pk_path,
            certificate: cert_path,
            ca_chain: ca_path,
        };

        print_json(&result)
    }
}
