use crate::result::{anyhow, bail, Result};
use helium_crypto::ledger;
use std::{path::PathBuf, str::FromStr};

/// A wallet source: either a path to an encrypted key file on disk, or a
/// reference to a key on a connected Ledger device.
///
/// Parsed from the `-f`/`--file` argument. Anything that doesn't start with
/// the `usb://ledger` prefix is treated as a file path (matching the existing
/// behaviour). A Ledger reference takes the form:
///
/// ```text
/// usb://ledger[?key=<account>[/<change>][&serial=<usb_serial>]]
/// ```
///
/// `key` defaults to `0/0` — the 4-level path `m/44'/501'/0'/0'` used by
/// Ledger Live and recent Phantom versions. A single component (`key=N`)
/// selects the 3-level Solana CLI path `m/44'/501'/N'`, which the Solana
/// CLI and older Phantom configurations use. Run `wallet ledger list` to
/// see which path family your funded accounts live on.
/// `serial` filters by USB serial when multiple Ledgers are attached.
#[derive(Debug, Clone)]
pub enum WalletSource {
    File(PathBuf),
    Ledger {
        path: ledger::DerivationPath,
        // Preserved for URL round-tripping; the Display form of `path` is
        // the BIP32 `m/...'` notation, not the `key=N[/M]` URL form.
        account: u32,
        change: Option<u32>,
        serial: Option<String>,
    },
}

const LEDGER_PREFIX: &str = "usb://ledger";

impl WalletSource {
    pub fn is_ledger(&self) -> bool {
        matches!(self, Self::Ledger { .. })
    }
}

impl FromStr for WalletSource {
    type Err = crate::result::Error;

    fn from_str(s: &str) -> Result<Self> {
        let Some(rest) = s.strip_prefix(LEDGER_PREFIX) else {
            return Ok(Self::File(PathBuf::from(s)));
        };

        let mut account: u32 = 0;
        let mut change: Option<u32> = Some(0);
        let mut serial = None;

        let query = match rest {
            "" => "",
            r if r.starts_with('?') => &r[1..],
            _ => bail!("invalid ledger url: {s}"),
        };

        if !query.is_empty() {
            for pair in query.split('&') {
                let (key, value) = pair
                    .split_once('=')
                    .ok_or_else(|| anyhow!("invalid ledger url query pair: {pair}"))?;
                match key {
                    "key" => (account, change) = parse_key_path(value)?,
                    "serial" => serial = Some(value.to_string()),
                    other => bail!("unknown ledger url query key: {other}"),
                }
            }
        }

        let path = match change {
            Some(change) => ledger::DerivationPath::solana(account, change),
            None => ledger::DerivationPath::solana_cli(account),
        };
        Ok(Self::Ledger {
            path,
            account,
            change,
            serial,
        })
    }
}

impl std::fmt::Display for WalletSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => write!(f, "{}", path.display()),
            Self::Ledger {
                account,
                change,
                serial,
                path: _,
            } => {
                write!(f, "{LEDGER_PREFIX}?key={account}")?;
                if let Some(change) = change {
                    write!(f, "/{change}")?;
                }
                if let Some(serial) = serial {
                    write!(f, "&serial={serial}")?;
                }
                Ok(())
            }
        }
    }
}

fn parse_key_path(s: &str) -> Result<(u32, Option<u32>)> {
    let parts: Vec<&str> = s.split('/').collect();
    let parse = |p: &str| -> Result<u32> {
        p.parse::<u32>()
            .map_err(|e| anyhow!("invalid ledger key component '{p}': {e}"))
    };
    match parts.as_slice() {
        [account] => Ok((parse(account)?, None)),
        [account, change] => Ok((parse(account)?, Some(parse(change)?))),
        _ => bail!("invalid ledger key path '{s}': expected <account> or <account>/<change>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_path_passthrough() {
        let src: WalletSource = "wallet.key".parse().unwrap();
        match src {
            WalletSource::File(p) => assert_eq!(p, PathBuf::from("wallet.key")),
            _ => panic!("expected File"),
        }
    }

    #[test]
    fn ledger_default_path() {
        let src: WalletSource = "usb://ledger".parse().unwrap();
        match src {
            WalletSource::Ledger {
                path,
                account,
                change,
                serial,
            } => {
                assert_eq!(path.to_string(), "m/44'/501'/0'/0'");
                assert_eq!(account, 0);
                assert_eq!(change, Some(0));
                assert!(serial.is_none());
            }
            _ => panic!("expected Ledger"),
        }
    }

    #[test]
    fn ledger_explicit_path() {
        let src: WalletSource = "usb://ledger?key=3/1".parse().unwrap();
        match src {
            WalletSource::Ledger { path, .. } => {
                assert_eq!(path.to_string(), "m/44'/501'/3'/1'");
            }
            _ => panic!("expected Ledger"),
        }
    }

    #[test]
    fn ledger_three_level_path() {
        let src: WalletSource = "usb://ledger?key=2".parse().unwrap();
        match src {
            WalletSource::Ledger { path, .. } => {
                assert_eq!(path.to_string(), "m/44'/501'/2'");
            }
            _ => panic!("expected Ledger"),
        }
    }

    #[test]
    fn ledger_serial() {
        let src: WalletSource = "usb://ledger?key=0/0&serial=ABC123".parse().unwrap();
        match src {
            WalletSource::Ledger { serial, .. } => {
                assert_eq!(serial.as_deref(), Some("ABC123"));
            }
            _ => panic!("expected Ledger"),
        }
    }

    #[test]
    fn ledger_unknown_query_param_rejected() {
        assert!("usb://ledger?foo=bar".parse::<WalletSource>().is_err());
    }

    #[test]
    fn ledger_malformed_url_rejected() {
        assert!("usb://ledgerstuff".parse::<WalletSource>().is_err());
    }

    #[test]
    fn ledger_url_roundtrips_via_display() {
        let s = "usb://ledger?key=1/0&serial=XYZ";
        let src: WalletSource = s.parse().unwrap();
        assert_eq!(src.to_string(), s);
    }
}
