//! Persistent cache for vault → multisig pubkey mappings. The relationship
//! is immutable (the multisig pubkey is a seed of the vault PDA), so cached
//! entries never need invalidation. The cache is best-effort: a missing or
//! unwritable file falls back to live resolution silently. A *corrupted*
//! file is preserved (not overwritten) and surfaced via `tracing::warn!`,
//! so a user can recover or hand-edit it instead of silently losing the
//! whole cache because one byte was mangled.

use crate::{
    error::{EncodeError, Error},
    keypair::Pubkey,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

/// Resolves to the platform's standard cache directory:
/// `~/Library/Caches/helium-wallet/...` on macOS,
/// `~/.cache/helium-wallet/...` (or `$XDG_CACHE_HOME/helium-wallet/...`) on
/// Linux, `%LOCALAPPDATA%\helium-wallet\...` on Windows.
fn cache_path() -> Option<PathBuf> {
    Some(
        dirs::cache_dir()?
            .join("helium-wallet")
            .join("squads-vaults.json"),
    )
}

/// Distinguish "no cache file yet" from "file exists but unreadable". The
/// `Corrupted` arm carries a reason string so `tracing::warn!` can render
/// the underlying io / parse error.
enum LoadOutcome {
    Loaded(BTreeMap<String, String>),
    Missing,
    Corrupted(String),
}

fn load(path: &Path) -> LoadOutcome {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Missing,
        Err(e) => return LoadOutcome::Corrupted(format!("io: {e}")),
    };
    match serde_json::from_str::<BTreeMap<String, String>>(&content) {
        Ok(m) => LoadOutcome::Loaded(m),
        Err(e) => LoadOutcome::Corrupted(format!("parse: {e}")),
    }
}

pub fn lookup(vault: &Pubkey) -> Option<Pubkey> {
    let path = cache_path()?;
    lookup_at(&path, vault)
}

fn lookup_at(path: &Path, vault: &Pubkey) -> Option<Pubkey> {
    match load(path) {
        LoadOutcome::Loaded(entries) => entries
            .get(&vault.to_string())
            .and_then(|s| Pubkey::from_str(s).ok()),
        LoadOutcome::Missing => None,
        LoadOutcome::Corrupted(reason) => {
            tracing::warn!(
                cache = %path.display(),
                reason = %reason,
                "squads vault cache corrupted; ignoring this session",
            );
            None
        }
    }
}

pub fn store(vault: &Pubkey, multisig: &Pubkey) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Err(error) = try_store_at(&path, vault, multisig) {
        // Cache writes are best-effort, but surface the failure at
        // debug level so a slow `--squads` workflow can be correlated
        // with cache breakage (disk full, permissions, etc.) rather
        // than re-running the 200-signature vault resolution scan
        // every time.
        tracing::debug!(
            vault = %vault,
            multisig = %multisig,
            error = %error,
            "squads vault cache write failed",
        );
    }
}

fn try_store_at(path: &Path, vault: &Pubkey, multisig: &Pubkey) -> Result<(), Error> {
    let mut entries = match load(path) {
        LoadOutcome::Loaded(m) => m,
        LoadOutcome::Missing => BTreeMap::new(),
        LoadOutcome::Corrupted(reason) => {
            // Don't silently overwrite a corrupted cache — a single
            // mangled byte shouldn't destroy a potentially large cache
            // the user could otherwise inspect or recover. Skip the
            // write; a healthy file from a future session will resume
            // accumulating entries.
            tracing::warn!(
                cache = %path.display(),
                reason = %reason,
                "squads vault cache corrupted; refusing to overwrite",
            );
            return Ok(());
        }
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(EncodeError::from)?;
    }
    entries.insert(vault.to_string(), multisig.to_string());
    let serialized = serde_json::to_string_pretty(&entries).map_err(EncodeError::from)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serialized).map_err(EncodeError::from)?;
    fs::rename(&tmp, path).map_err(EncodeError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Round-trip: a `try_store_at` followed by `lookup_at` returns
    /// the stored multisig. Pins the basic happy path.
    #[test]
    fn store_then_lookup_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("squads-vaults.json");
        let vault = Pubkey::new_unique();
        let multisig = Pubkey::new_unique();

        try_store_at(&path, &vault, &multisig).expect("store");
        assert_eq!(lookup_at(&path, &vault), Some(multisig));
    }

    /// Multiple `try_store_at` calls accumulate without losing prior
    /// entries — `try_store_at` reads the existing file, merges the
    /// new entry, and writes back.
    #[test]
    fn store_accumulates() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("squads-vaults.json");
        let (v1, m1) = (Pubkey::new_unique(), Pubkey::new_unique());
        let (v2, m2) = (Pubkey::new_unique(), Pubkey::new_unique());

        try_store_at(&path, &v1, &m1).expect("store 1");
        try_store_at(&path, &v2, &m2).expect("store 2");

        assert_eq!(lookup_at(&path, &v1), Some(m1));
        assert_eq!(lookup_at(&path, &v2), Some(m2));
    }

    /// A missing cache file is indistinguishable from "no entry" —
    /// `lookup_at` returns None without panicking, and `try_store_at`
    /// creates the file from scratch.
    #[test]
    fn missing_file_is_clean_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("squads-vaults.json");
        assert_eq!(lookup_at(&path, &Pubkey::new_unique()), None);

        let vault = Pubkey::new_unique();
        let multisig = Pubkey::new_unique();
        try_store_at(&path, &vault, &multisig).expect("store");
        assert!(path.exists());
        assert_eq!(lookup_at(&path, &vault), Some(multisig));
    }

    /// Corrupted JSON: `lookup_at` returns None (logging a warn), and
    /// — critically — `try_store_at` refuses to overwrite the file.
    /// Pins the "best-effort but never destroy" contract.
    #[test]
    fn corruption_preserves_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("squads-vaults.json");
        let original = "{not valid json"; // intentionally truncated
        fs::write(&path, original).unwrap();

        // Lookups don't panic and don't return data.
        assert_eq!(lookup_at(&path, &Pubkey::new_unique()), None);

        // Store doesn't error (best-effort) but also doesn't
        // overwrite — the corrupted contents survive for inspection.
        try_store_at(&path, &Pubkey::new_unique(), &Pubkey::new_unique()).expect("store skips");
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }
}
