use crate::cmd::*;
use chrono::{Duration, Utc};
use helium_lib::{keypair::Pubkey, squads};

/// List actionable proposals on a Squads multisig — Active and
/// Approved (v4) / ExecuteReady (v3) statuses, ordered newest first.
/// Drafts are hidden by default since they're pre-activation parking
/// and an old draft is almost always abandoned context; pass
/// `--include-drafts` to surface them. Defaults to the last 7 days
/// of activity — old still-open proposals are usually abandoned
/// context regardless of status; widen the window with `--days <N>`
/// or pass `--days 0` to disable the age filter entirely.
/// Finalized proposals (Executed, Rejected, Cancelled) and v4
/// stale-by-config proposals are never shown — those can't be acted on.
#[derive(Debug, Clone, clap::Args)]
pub struct Cmd {
    /// Multisig, vault, or any transaction/proposal PDA in the multisig.
    target: Pubkey,

    /// Also include Draft proposals. Drafts are pre-activation
    /// parking — useful when triaging your own pending work, noise
    /// when triaging votes or executions someone else needs to drive.
    #[arg(long)]
    include_drafts: bool,

    /// Look back this many days; older proposals are filtered out.
    /// `0` disables the filter and shows every still-open proposal
    /// regardless of age. v3 multisigs don't carry per-status
    /// timestamps — those entries pass through unconditionally.
    #[arg(long, default_value_t = 7)]
    days: u32,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let client = opts.client()?;
        let entries = squads::list_open_proposals(&client, &self.target).await?;
        let cutoff = (self.days > 0).then(|| Utc::now() - Duration::days(i64::from(self.days)));
        let filtered: Vec<_> = entries
            .into_iter()
            .filter(|e| self.include_drafts || e.status != "draft")
            .filter(|e| match (cutoff, e.status_timestamp) {
                // No filter requested or v3 entry without a timestamp:
                // pass through.
                (None, _) | (_, None) => true,
                (Some(cutoff), Some(ts)) => ts >= cutoff,
            })
            .collect();
        print_json(&filtered)
    }
}
