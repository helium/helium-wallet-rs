use crate::cmd::*;
use helium_lib::{
    blockchain_api::types::UpdateRewardsDestinationRequest, entity_key, keypair::Signer, reward,
    reward::ClaimableToken,
};

#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: RewardsCommand,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

/// Inspect or claim rewards for assets
#[derive(Debug, Clone, clap::Subcommand)]
pub enum RewardsCommand {
    Claim(assets::claim::Cmd),
    Pending(PendingCmd),
    Recipient(RecipientCmd),
    Lifetime(LifetimeCmd),
    MaxClaim(MaxClaimCmd),
}

impl RewardsCommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Claim(cmd) => cmd.run(opts).await,
            Self::Recipient(cmd) => cmd.run(opts).await,
            Self::MaxClaim(cmd) => cmd.run(opts).await,
            Self::Pending(cmd) => cmd.run(opts).await,
            Self::Lifetime(cmd) => cmd.run(opts).await,
        }
    }
}

/// Manage the recipient for rewards
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientCmd {
    #[command(subcommand)]
    cmd: RecipientSubcommand,
}

impl RecipientCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum RecipientSubcommand {
    Get(RecipientGetCmd),
    Init(RecipientInitCmd),
    Update(RecipientUpdateCmd),
}

impl RecipientSubcommand {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Get(cmd) => cmd.run(opts).await,
            Self::Init(cmd) => cmd.run(opts).await,
            Self::Update(cmd) => cmd.run(opts).await,
        }
    }
}

/// Get the current reward recipient destination for an asset
///
/// Returns the wallet address where rewards for this asset will be sent
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientGetCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    /// The asset to get the reward recipient for
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
}

impl RecipientGetCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let destination = reward::recipient::destination::for_entity_key(
            &client,
            self.token,
            &self.entity_key.as_entity_key()?,
        )
        .await?;
        let json = json!({
            "destination": destination.to_string(),
        });
        print_json(&json)
    }
}

/// Initialize the recipient for an asset
///
/// Creates the on-chain recipient account for an asset, with the reward
/// destination set to your wallet. Required before rewards can be claimed or a
/// custom destination set.
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientInitCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    /// The asset to initialize the reward recipient for
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    #[command(flatten)]
    pub commit: CommitOpts,
}

impl RecipientInitCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let signer = opts.load_signer()?;
        let api = opts.blockchain_api()?;
        // The update-rewards-destination endpoint creates the recipient account
        // if it doesn't exist, so initializing is just setting the destination
        // to the caller's own wallet.
        let response = api
            .update_rewards_destination(&UpdateRewardsDestinationRequest {
                wallet_address: signer.pubkey().to_string(),
                hotspot_pubkey: self.entity_key.to_string(),
                destination: signer.pubkey().to_string(),
                lazy_distributors: vec![self.token.lazy_distributor_key().to_string()],
            })
            .await?;
        // Initializing points rewards at this wallet, so it is the same
        // standing redirect the update path sets and is held to the same check.
        let wallet = signer.pubkey();
        assert_updates_destination(&response.decode_transactions()?, &wallet, &wallet)?;
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

/// Update the reward recipient destination for an asset
///
/// Changes where rewards for this asset will be sent. The recipient account will
/// be initialized if it doesn't already exist.
#[derive(Debug, Clone, clap::Args)]
pub struct RecipientUpdateCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    pub token: ClaimableToken,
    /// The asset to update the reward recipient for
    #[clap(flatten)]
    pub entity_key: entity_key::EncodedEntityKey,
    /// The new destination wallet address to send rewards to
    pub destination: helium_lib::keypair::Pubkey,
    #[command(flatten)]
    pub commit: CommitOpts,
}

/// Instructions that move where a hotspot's rewards are paid. Both put the new
/// destination at account index 2 and the owner-signer at index 1.
const UPDATE_DESTINATION_METHODS: &[&str] =
    &["update_destination_v0", "update_compression_destination_v0"];
const DESTINATION_ACCOUNT_INDEX: usize = 2;
const OWNER_ACCOUNT_INDEX: usize = 1;

/// Refuse to sign unless the transaction redirects rewards to the destination
/// that was asked for, on behalf of this wallet.
///
/// A substituted destination is the highest-value thing the API can return:
/// unlike a one-off transfer it is a standing redirect that keeps paying after
/// the compromise is found, and it needs a second on-chain action to undo. The
/// review summary cannot separate the two cases, since both invoke the same
/// program.
fn assert_updates_destination(
    unsigned: &[helium_lib::transaction::VersionedTransaction],
    wallet: &Pubkey,
    destination: &Pubkey,
) -> Result {
    use helium_lib::{programs::KnownProgram, verify};
    let found = verify::find_methods(
        unsigned,
        KnownProgram::LazyDistributor,
        UPDATE_DESTINATION_METHODS,
    )?;
    if found.is_empty() {
        bail!("no rewards-destination update found in the transaction; refusing to sign");
    }
    for ix in &found {
        let method = ix.method;
        let got = ix.account(DESTINATION_ACCOUNT_INDEX)?;
        if got != *destination {
            bail!("{method} would send rewards to {got}, not the requested {destination}");
        }
        let owner = ix.account(OWNER_ACCOUNT_INDEX)?;
        if owner != *wallet {
            bail!("{method} is authorized by {owner}, not this wallet");
        }
    }
    Ok(())
}

impl RecipientUpdateCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let signer = opts.load_signer()?;
        let api = opts.blockchain_api()?;
        // The server resolves `hotspotPubkey` to an asset id; for a hotspot
        // entity key this is its base58 helium public key. The token selects
        // which lazy distributor's recipient destination to update.
        let response = api
            .update_rewards_destination(&UpdateRewardsDestinationRequest {
                wallet_address: signer.pubkey().to_string(),
                hotspot_pubkey: self.entity_key.to_string(),
                destination: self.destination.to_string(),
                lazy_distributors: vec![self.token.lazy_distributor_key().to_string()],
            })
            .await?;
        assert_updates_destination(
            &response.decode_transactions()?,
            &signer.pubkey(),
            &self.destination,
        )?;
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
/// List the maximum claim amount for the given subdao
///
/// The max claim amoount is the configured threshold for the subdao, adjusted down by a time
/// decayed amount bed on previous claims
pub struct MaxClaimCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: ClaimableToken,
}

impl MaxClaimCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let max_claim = reward::max_claim(&client, self.token).await?;
        print_json(&max_claim)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List claimable pending rewards for a given asset
pub struct PendingCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: ClaimableToken,
    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
}

impl PendingCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let pending =
            reward::pending_amounts(&client, self.token, None, &[&self.entity_key]).await?;

        print_json(&pending)
    }
}

#[derive(Debug, Clone, clap::Args)]
/// List lifetime rewards for an asset
///
/// This includes both claimed and unclaimed rewards
pub struct LifetimeCmd {
    /// Token for command
    #[clap(long, default_value_t)]
    token: ClaimableToken,
    #[clap(flatten)]
    entity_key: entity_key::EncodedEntityKey,
}

impl LifetimeCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let rewards = reward::lifetime(&client, self.token, &[&self.entity_key]).await?;

        print_json(&rewards)
    }
}

#[cfg(test)]
mod destination_guard_tests {
    use super::*;
    use helium_lib::programs::KnownProgram;
    use helium_lib::solana_sdk::instruction::CompiledInstruction;
    use helium_lib::solana_sdk::message::{Message, MessageHeader, VersionedMessage};
    use helium_lib::transaction::VersionedTransaction;

    /// Anchor discriminator for `update_destination_v0`.
    const UPDATE_DESTINATION_V0: [u8; 8] = [196, 237, 208, 178, 104, 7, 36, 14];

    /// A transaction whose sole instruction is an update-destination with the
    /// given owner and destination in their IDL account slots.
    fn update_tx(owner: Pubkey, destination: Pubkey) -> VersionedTransaction {
        let recipient = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let keys = vec![
            owner,
            KnownProgram::LazyDistributor.id(),
            recipient,
            destination,
            mint,
        ];
        VersionedTransaction {
            signatures: vec![Default::default()],
            message: VersionedMessage::Legacy(Message {
                header: MessageHeader {
                    num_required_signatures: 1,
                    num_readonly_signed_accounts: 0,
                    num_readonly_unsigned_accounts: 4,
                },
                account_keys: keys,
                recent_blockhash: Default::default(),
                instructions: vec![CompiledInstruction {
                    program_id_index: 1,
                    // recipient, owner, destination, recipient_mint_account
                    accounts: vec![2, 0, 3, 4],
                    data: UPDATE_DESTINATION_V0.to_vec(),
                }],
            }),
        }
    }

    #[test]
    fn accepts_the_requested_destination() {
        let (wallet, destination) = (Pubkey::new_unique(), Pubkey::new_unique());
        assert_updates_destination(&[update_tx(wallet, destination)], &wallet, &destination)
            .expect("the requested destination must be accepted");
    }

    #[test]
    fn refuses_a_substituted_destination() {
        let (wallet, wanted, attacker) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let err = assert_updates_destination(&[update_tx(wallet, attacker)], &wallet, &wanted)
            .expect_err("a substituted destination must be refused");
        let msg = err.to_string();
        assert!(msg.contains(&attacker.to_string()), "{msg}");
        assert!(msg.contains(&wanted.to_string()), "{msg}");
    }

    #[test]
    fn refuses_an_update_authorized_by_another_wallet() {
        let (wallet, other, destination) = (
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );
        let err =
            assert_updates_destination(&[update_tx(other, destination)], &wallet, &destination)
                .expect_err("another authority must be refused");
        assert!(err.to_string().contains("authorized by"), "{err}");
    }

    #[test]
    fn refuses_a_transaction_with_no_update_at_all() {
        // Signing something that is not the action asked for is the case a
        // program-name review line cannot distinguish.
        let (wallet, destination) = (Pubkey::new_unique(), Pubkey::new_unique());
        let mut tx = update_tx(wallet, destination);
        if let VersionedMessage::Legacy(msg) = &mut tx.message {
            msg.instructions[0].data = vec![9; 8];
        }
        let err = assert_updates_destination(&[tx], &wallet, &destination)
            .expect_err("a transaction that does not update the destination must be refused");
        assert!(
            err.to_string().contains("no rewards-destination update"),
            "{err}"
        );
    }
}
