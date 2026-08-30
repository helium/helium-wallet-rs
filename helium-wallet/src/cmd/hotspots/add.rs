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
    programs::KnownProgram,
    transaction::VersionedTransaction,
    verify,
};
use helium_proto::{BlockchainTxn, BlockchainTxnAddGatewayV1, Message, Txn};
use rand::rngs::OsRng;
use serde::Serialize;
use std::{io::Write, time::Duration};

/// `issue_data_only_entity_v0` names the wallet the new hotspot is minted to at
/// account index 10.
const ISSUE_METHODS: &[&str] = &["issue_data_only_entity_v0"];
const ISSUE_RECIPIENT_INDEX: usize = 10;

/// Both onboard instructions name the hotspot's owner at account index 3 and
/// carry the asserted location in their arguments.
const ONBOARD_METHODS: &[&str] = &[
    "onboard_data_only_iot_hotspot_v0",
    "onboard_data_only_mobile_hotspot_v0",
];
const ONBOARD_OWNER_INDEX: usize = 3;

/// Refuse to sign unless the hotspot is minted to this wallet.
///
/// The recipient is the whole of what issuing decides: a hotspot minted to
/// another wallet is that wallet's, and the add-gateway token it was minted
/// from cannot be spent twice to correct it.
fn assert_issues_to_wallet(unsigned: &[VersionedTransaction], wallet: &Pubkey) -> Result<()> {
    let found = verify::find_methods(unsigned, KnownProgram::HeliumEntityManager, ISSUE_METHODS)?;
    let [issue] = found.as_slice() else {
        bail!(
            "expected exactly one hotspot issue, found {}; refusing to sign",
            found.len()
        );
    };
    let recipient = issue.account(ISSUE_RECIPIENT_INDEX)?;
    if recipient != *wallet {
        bail!("the hotspot would be issued to {recipient}, not this wallet");
    }
    Ok(())
}

/// Refuse to sign unless the onboard is for a hotspot this wallet owns, at the
/// location that was asserted.
///
/// Location is compared exactly, from the same coordinates that were sent.
/// Elevation and gain are left to the service's own unit conversion, as
/// duplicating its rounding here would refuse honest onboards the day it
/// changes.
fn assert_onboards_to_wallet(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    lat: Option<f64>,
    lon: Option<f64>,
) -> Result<()> {
    let found = verify::find_methods(unsigned, KnownProgram::HeliumEntityManager, ONBOARD_METHODS)?;
    let [onboard] = found.as_slice() else {
        bail!(
            "expected exactly one hotspot onboard, found {}; refusing to sign",
            found.len()
        );
    };
    let owner = onboard.account(ONBOARD_OWNER_INDEX)?;
    if owner != *wallet {
        bail!("the onboard is for a hotspot owned by {owner}, not this wallet");
    }
    let args = onboard
        .args()
        .ok_or_else(|| anyhow!("the onboard's arguments could not be read; refusing to sign"))?;
    let want = hotspot::cell_for(lat, lon)?.map(u64::from);
    let got = args["args"]["location"].as_u64();
    if got != want {
        bail!("the onboard asserts location {got:?}, not the requested {want:?}");
    }
    Ok(())
}

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
        assert_issues_to_wallet(&response.decode_transactions()?, &signer.pubkey())?;
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
        assert_onboards_to_wallet(
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

    /// Discriminators the entity-manager IDL declares for these methods.
    const ISSUE: [u8; 8] = [191, 96, 245, 46, 63, 73, 207, 17];
    const ONBOARD_IOT: [u8; 8] = [98, 179, 127, 51, 58, 191, 174, 188];

    const LAT: f64 = 37.7749;
    const LON: f64 = -122.4194;

    fn anchor_tx(
        discriminator: [u8; 8],
        body: Vec<u8>,
        accounts: &[Pubkey],
    ) -> VersionedTransaction {
        let mut data = discriminator.to_vec();
        data.extend_from_slice(&body);
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
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&accounts[0]))),
        }
    }

    /// `recipient` sits at index 10, so the first ten are filler.
    fn issue_tx(recipient: Pubkey) -> VersionedTransaction {
        let mut accounts: Vec<Pubkey> = (0..10).map(|_| Pubkey::new_unique()).collect();
        accounts.push(recipient);
        anchor_tx(ISSUE, vec![], &accounts)
    }

    /// `OnboardDataOnlyIotHotspotArgsV0`: the proof fields, then the options.
    fn onboard_tx(owner: Pubkey, location: Option<u64>) -> VersionedTransaction {
        let mut body = Vec::new();
        body.extend_from_slice(&[0u8; 32]); // data_hash
        body.extend_from_slice(&[0u8; 32]); // creator_hash
        body.extend_from_slice(&[0u8; 32]); // root
        body.extend_from_slice(&0u32.to_le_bytes()); // index
        match location {
            Some(v) => {
                body.push(1);
                body.extend_from_slice(&v.to_le_bytes());
            }
            None => body.push(0),
        }
        body.push(0); // elevation: None
        body.push(0); // gain: None
        let accounts = [
            Pubkey::new_unique(), // payer
            Pubkey::new_unique(), // dc_fee_payer
            Pubkey::new_unique(), // iot_info
            owner,
        ];
        anchor_tx(ONBOARD_IOT, body, &accounts)
    }

    fn cell(lat: f64, lon: f64) -> u64 {
        u64::from(
            hotspot::cell_for(Some(lat), Some(lon))
                .expect("a valid coordinate")
                .expect("a cell"),
        )
    }

    #[test]
    fn a_hotspot_issued_to_this_wallet_is_accepted() {
        let wallet = Pubkey::new_unique();
        assert_issues_to_wallet(&[issue_tx(wallet)], &wallet).expect("issued to this wallet");
    }

    #[test]
    fn a_hotspot_issued_to_another_wallet_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_issues_to_wallet(&[issue_tx(Pubkey::new_unique())], &wallet)
            .expect_err("a substituted recipient must be refused");
        assert!(err.to_string().contains("not this wallet"), "{err}");
    }

    #[test]
    fn a_response_carrying_no_issue_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_issues_to_wallet(&[], &wallet)
            .expect_err("an action that was never built must be refused");
        assert!(err.to_string().contains("found 0"), "{err}");
    }

    #[test]
    fn a_batch_issuing_twice_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_issues_to_wallet(&[issue_tx(wallet), issue_tx(wallet)], &wallet)
            .expect_err("a batch issuing twice must be refused");
        assert!(err.to_string().contains("found 2"), "{err}");
    }

    #[test]
    fn an_onboard_at_the_asserted_location_is_accepted() {
        let wallet = Pubkey::new_unique();
        assert_onboards_to_wallet(
            &[onboard_tx(wallet, Some(cell(LAT, LON)))],
            &wallet,
            Some(LAT),
            Some(LON),
        )
        .expect("the asserted location");
    }

    #[test]
    fn an_onboard_at_another_location_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_onboards_to_wallet(
            &[onboard_tx(wallet, Some(cell(51.5074, -0.1278)))],
            &wallet,
            Some(LAT),
            Some(LON),
        )
        .expect_err("a substituted location must be refused");
        assert!(err.to_string().contains("not the requested"), "{err}");
    }

    #[test]
    fn an_onboard_for_another_owner_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_onboards_to_wallet(
            &[onboard_tx(Pubkey::new_unique(), Some(cell(LAT, LON)))],
            &wallet,
            Some(LAT),
            Some(LON),
        )
        .expect_err("another owner must be refused");
        assert!(err.to_string().contains("not this wallet"), "{err}");
    }

    #[test]
    fn a_batch_onboarding_twice_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_onboards_to_wallet(
            &[
                onboard_tx(wallet, Some(cell(LAT, LON))),
                onboard_tx(wallet, Some(cell(LAT, LON))),
            ],
            &wallet,
            Some(LAT),
            Some(LON),
        )
        .expect_err("a batch onboarding twice must be refused");
        assert!(err.to_string().contains("found 2"), "{err}");
    }

    #[test]
    fn an_onboard_asserting_a_location_that_was_not_requested_is_refused() {
        let wallet = Pubkey::new_unique();
        let err = assert_onboards_to_wallet(
            &[onboard_tx(wallet, Some(cell(LAT, LON)))],
            &wallet,
            None,
            None,
        )
        .expect_err("an unrequested assertion must be refused");
        assert!(err.to_string().contains("not the requested"), "{err}");
    }
}
