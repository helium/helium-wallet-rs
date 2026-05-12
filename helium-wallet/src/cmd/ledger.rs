use crate::cmd::*;
use helium_crypto::{ledger, Network};
use helium_lib::{
    keypair::{to_helium_pubkey, to_pubkey, Pubkey},
    token::{self, Token, TokenBalanceMap},
};

/// Operations on a connected Ledger device.
#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: SubCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum SubCmd {
    List(ListCmd),
    Devices(DevicesCmd),
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match &self.cmd {
            SubCmd::List(cmd) => cmd.run(opts).await,
            SubCmd::Devices(cmd) => cmd.run().await,
        }
    }
}

/// Enumerate every Ledger device USB-HID can see — useful for finding a
/// device's USB serial to pass via `usb://ledger?serial=...` when multiple
/// Ledgers are attached.
#[derive(Debug, clap::Args)]
pub struct DevicesCmd;

impl DevicesCmd {
    pub async fn run(&self) -> Result {
        let devices = ledger::list_devices()?;
        print_json(&json!({ "devices": devices }))
    }
}

/// Enumerate Solana-derivation accounts on a connected Ledger and annotate
/// each with its on-chain balances.
///
/// Both Solana derivation-path families are scanned by default:
/// - 4-level `m/44'/501'/N'/0'` — Ledger Live and recent Phantom default.
/// - 3-level `m/44'/501'/N'` — Solana CLI, older Phantom, some other wallets.
///
/// The two families produce different addresses for the same account index;
/// scanning both ensures we don't miss a funded account just because Phantom
/// happened to use one form when the user originally connected the device.
///
/// Funded accounts are listed first; empty accounts are hidden unless
/// `--all`. Each entry includes a copy-pasteable `usb://ledger?key=...`
/// URL that can be passed to `--file`. Requires the Solana app to be open
/// on the device.
#[derive(Debug, clap::Args)]
pub struct ListCmd {
    /// Number of accounts to enumerate, starting from account 0.
    #[arg(long, default_value_t = 10)]
    count: u32,

    /// Include accounts with no balance. Default hides empty accounts when
    /// balances are available.
    #[arg(long)]
    all: bool,

    /// Skip the on-chain balance lookup. Useful for offline / air-gapped
    /// derivation when no RPC is reachable.
    #[arg(long)]
    no_balance: bool,

    /// USB serial of the Ledger to scan. When omitted, the first connected
    /// device is opened.
    #[arg(long)]
    serial: Option<String>,
}

struct Entry {
    url: String,
    path: String,
    solana: Pubkey,
    helium: helium_crypto::PublicKey,
    balance: Option<TokenBalanceMap>,
}

impl Entry {
    fn is_funded(&self) -> bool {
        self.balance
            .as_ref()
            .is_some_and(|b| !b.as_ref().is_empty())
    }

    fn to_json(&self) -> serde_json::Value {
        let mut value = json!({
            "url": self.url,
            "path": self.path,
            "address": {
                "solana": self.solana.to_string(),
                "helium": self.helium.to_string(),
            },
        });
        if let Some(balance) = &self.balance {
            value["balance"] = serde_json::to_value(balance).unwrap_or(json!({}));
        }
        value
    }
}

impl ListCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        // 1. Derive accounts on the device — both 3-level and 4-level path
        //    families per index, since Phantom/Ledger Live/Solana CLI vary
        //    across versions and configurations.
        let serial = self.serial.as_deref();
        let mut entries: Vec<Entry> = Vec::with_capacity(self.count as usize * 2);
        for n in 0..self.count {
            entries.push(derive(
                ledger::DerivationPath::solana(n, 0),
                n,
                Some(0),
                serial,
            )?);
            entries.push(derive(
                ledger::DerivationPath::solana_cli(n),
                n,
                None,
                serial,
            )?);
        }

        // 2. Bulk-fetch balances unless explicitly disabled. On RPC failure,
        //    fall back to address-only output with a warning so the discovery
        //    flow still works in degraded conditions.
        let mut warning: Option<String> = None;
        if !self.no_balance {
            match fetch_balances(&opts, &entries).await {
                Ok(balances) => {
                    for (entry, balance) in entries.iter_mut().zip(balances) {
                        entry.balance = Some(balance);
                    }
                }
                Err(err) => {
                    warning = Some(format!("balance lookup failed: {err}"));
                }
            }
        }

        // 3. Sort funded first when balances are available; otherwise keep
        //    derivation order.
        let have_balances = !self.no_balance && warning.is_none();
        if have_balances {
            entries.sort_by_key(|e| !e.is_funded());
        }

        // 4. Hide empty accounts unless --all or balances unavailable.
        if have_balances && !self.all {
            entries.retain(Entry::is_funded);
        }

        // 5. Emit JSON.
        let accounts: Vec<_> = entries.iter().map(Entry::to_json).collect();
        let mut out = json!({ "accounts": accounts });
        if let Some(w) = warning {
            out["warning"] = json!(w);
        }
        print_json(&out)
    }
}

fn derive(
    path: ledger::DerivationPath,
    account: u32,
    change: Option<u32>,
    serial: Option<&str>,
) -> Result<Entry> {
    let kp = ledger::Keypair::from_derivation_path(Network::MainNet, path.clone(), serial)?;
    let solana = to_pubkey(&kp.public_key)?;
    let helium = to_helium_pubkey(&solana)?;
    let url = match change {
        Some(change) => format!("usb://ledger?key={account}/{change}"),
        None => format!("usb://ledger?key={account}"),
    };
    Ok(Entry {
        url,
        path: path.to_string(),
        solana,
        helium,
        balance: None,
    })
}

/// Bulk-query SOL + Helium ATA balances for every entry in one
/// getMultipleAccounts roundtrip (chunked internally to Solana's 100/call cap).
async fn fetch_balances(opts: &Opts, entries: &[Entry]) -> Result<Vec<TokenBalanceMap>> {
    let client = opts.client()?;
    let tokens_per_owner = Token::all().len();

    let mut atas: Vec<Pubkey> = Vec::with_capacity(entries.len() * tokens_per_owner);
    for entry in entries {
        atas.extend(Token::associated_token_addresses(&entry.solana));
    }

    let balances = token::balance_for_addresses(&client, &atas).await?;
    Ok(balances
        .chunks(tokens_per_owner)
        .map(|chunk| TokenBalanceMap::from(chunk.to_vec()))
        .collect())
}
