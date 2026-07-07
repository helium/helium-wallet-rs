use crate::cmd::*;
use helium_lib::{blockchain_api::types::MemoRequest, keypair::Signer};

/// Send a memo to the blockchain
#[derive(Debug, clap::Args)]
pub struct Cmd {
    /// message to send.
    ///
    /// Remain under about 500 bytes for the message
    message: String,
    /// Commit the message
    #[command(flatten)]
    commit: CommitOpts,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let api = opts.blockchain_api()?;
        let response = api
            .memo(&MemoRequest {
                wallet_address: signer.pubkey().to_string(),
                memo: self.message.clone(),
            })
            .await?;
        print_json(
            &self
                .commit
                .commit_via_api(&api, &client, &response, &*signer)
                .await?
                .to_json(),
        )
    }
}
