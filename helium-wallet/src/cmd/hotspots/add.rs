use crate::{cmd::*, txn_envelope::TxnEnvelope};
use helium_crypto::KeyTag;
use helium_lib::{
    asset,
    client::{VERIFIER_URL_DEVNET, VERIFIER_URL_MAINNET},
    dao::SubDao,
    hotspot::{self, HotspotInfoUpdate},
};
use helium_proto::BlockchainTxnAddGatewayV1;
use rand::rngs::OsRng;

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

/// Add an IOT hotspot to the blockchain.
///
/// The required transaction is created by a
/// hotspot and supplied here for owner signing.
#[derive(Clone, Debug, clap::Args)]
struct IotCmd {
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

    /// The antenna gain for the asserted IoT hotspotin dBi, with one digit of
    /// accuracy.
    ///
    /// Defaults to the last asserted value. Note that the gain is truncated to
    /// the nearest 0.1 dBi.
    #[arg(long)]
    gain: Option<f64>,

    /// The elevation for the asserted IoT hotspot in meters above ground level.
    ///
    /// Defaults to the last assserted value. For negative values use '=', for
    /// example: "--elevation=-xx".
    #[arg(long)]
    elevation: Option<i32>,

    /// Base64 encoded hotspot transaction.
    txn: Transaction,

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

    if !hotspot_issued {
        let (tx, _) = hotspot::dataonly::issue(&client, verifier, &mut txn, &keypair).await?;
        let response = commit.maybe_commit(&tx, &client).await?;
        print_json(&response.to_json())?;
    }
    // Only assert the hotspot if either (a) it has already been issued before this cli
    // was run or (b) `commit` is enabled which means the previous command should have created it.
    // Without this, the command will always fail for brand new hotspots when --commit is not
    // enabled, as it cannot find the key_to_asset account or asset account.
    if hotspot_issued || commit.commit {
        let (tx, _) =
            hotspot::dataonly::onboard(&client, subdao, &gateway, update, &keypair).await?;
        print_json(&commit.maybe_commit(&tx, &client).await?.to_json())
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

/// Add a MOBILE hotspot to the blockchain.
///
/// The required transaction is created by using the 'txn' subcommand
/// hotspot and supplied here for owner signing.i
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
}

impl MobileCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Token(cmd) => cmd.run(opts).await,
            Self::Onboard(cmd) => cmd.run(opts).await,
        }
    }
}

/// Create an onboarding transaction for a mobile dataonly hotspot
#[derive(Debug, Clone, clap::Args)]
struct MobileToken {}

impl MobileToken {
    pub async fn run(&self, _opts: Opts) -> Result {
        let gw_keypair = helium_crypto::Keypair::generate(KeyTag::default(), &mut OsRng);
        let issue_token = hotspot::dataonly::issue_token(&gw_keypair)?;
        print_json(&issue_token)
    }
}

/// Obboard the given hotspot given a transaction
///
/// Issues the mobile hotspot NNFT, and onboards it given the created dataonly transaction.
/// Location details are optional
#[derive(Debug, Clone, clap::Args)]
struct MobileOnboard {
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
    /// Base64 encoded add hotspot token.
    ///
    /// The token is generated by the 'token' command
    token: Transaction,

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
