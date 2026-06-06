//! Named-contact address book.
//!
//! Persisted as JSON at `$XDG_CONFIG_HOME/helium-wallet/contacts.json`
//! (overridable via `HELIUM_WALLET_CONTACTS`). A contact name can be
//! used anywhere an address is accepted on the command line — a literal
//! base58 pubkey always wins over a same-named contact, so a malicious
//! contacts file can never redirect a hand-typed address.

use crate::result::{anyhow, bail, Result};
use helium_lib::keypair::{serde_pubkey, Pubkey};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::OnceLock,
};

const ENV_VAR: &str = "HELIUM_WALLET_CONTACTS";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContactBook {
    #[serde(default)]
    pub contacts: Vec<Contact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    #[serde(with = "serde_pubkey")]
    pub address: Pubkey,
}

impl ContactBook {
    /// Resolve the on-disk path: `$HELIUM_WALLET_CONTACTS` if set,
    /// otherwise `$XDG_CONFIG_HOME/helium-wallet/contacts.json` with the
    /// usual `$HOME/.config` fallback.
    pub fn default_path() -> Result<PathBuf> {
        if let Some(path) = env::var_os(ENV_VAR).filter(|s| !s.is_empty()) {
            return Ok(PathBuf::from(path));
        }
        let base = match env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from(env::var_os("HOME").ok_or_else(|| anyhow!("$HOME not set"))?)
                .join(".config"),
        };
        Ok(base.join("helium-wallet").join("contacts.json"))
    }

    /// Load from disk. A missing file yields an empty book — users who
    /// never create one see no errors.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Atomic write: temp file in the same dir, then rename. The tmp
    /// file is cleaned up on any failure after it's been written so
    /// cross-filesystem `rename` errors or a crash mid-rename don't
    /// leave a `.tmp` artifact next to the real file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        if let Err(err) = fs::write(&tmp, &bytes) {
            let _ = fs::remove_file(&tmp);
            return Err(err.into());
        }
        if let Err(err) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(err.into());
        }
        Ok(())
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.name == name)
    }

    pub fn find_by_address(&self, address: &Pubkey) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.address == *address)
    }

    pub fn add(&mut self, contact: Contact) -> Result<()> {
        let name = &contact.name;
        if name.is_empty() {
            bail!("contact name must not be empty");
        }
        if name.chars().any(char::is_whitespace) {
            bail!("contact name '{name}' contains whitespace; names must be a single CLI argument");
        }
        if Pubkey::from_str(name).is_ok() {
            bail!("contact name '{name}' parses as a Solana address; pick a non-pubkey name");
        }
        if self.find_by_name(name).is_some() {
            bail!("contact named '{name}' already exists");
        }
        self.contacts.push(contact);
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<Contact> {
        let idx = self
            .contacts
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| anyhow!("no contact named '{name}'"))?;
        Ok(self.contacts.remove(idx))
    }
}

/// Process-wide cached read of the default book. Loads lazily on first
/// use; a missing or malformed file degrades to an empty book so name
/// resolution can never break commands that don't use the feature.
pub fn cached() -> &'static ContactBook {
    static BOOK: OnceLock<ContactBook> = OnceLock::new();
    BOOK.get_or_init(|| {
        ContactBook::default_path()
            .and_then(|p| ContactBook::load(&p))
            .unwrap_or_default()
    })
}

/// Resolve `input` against an explicit book. Literal Solana base58
/// pubkeys are tried first — a contact with the same name as a real
/// address can never shadow that address. Factored out from
/// `parse_address_or_name` so tests can pin the precedence rule
/// without going through the process-wide `OnceLock`.
pub fn resolve_with(book: &ContactBook, input: &str) -> Result<Pubkey> {
    if let Ok(pk) = Pubkey::from_str(input) {
        return Ok(pk);
    }
    match book.find_by_name(input) {
        Some(c) => Ok(c.address),
        None => bail!("'{input}' is not a known contact or a valid Solana address"),
    }
}

/// clap value parser for any field that accepts either a literal Solana
/// base58 pubkey or the name of a known contact. Delegates to
/// `resolve_with` against the process-cached book.
pub fn parse_address_or_name(input: &str) -> Result<Pubkey> {
    resolve_with(cached(), input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn pk(seed: u8) -> Pubkey {
        Pubkey::new_from_array([seed; 32])
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("contacts.json");
        let book = ContactBook::load(&path).expect("load missing");
        assert!(book.contacts.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("sub").join("contacts.json");
        let mut book = ContactBook::default();
        book.add(Contact {
            name: "alice".to_string(),
            address: pk(1),
        })
        .expect("add alice");
        book.save(&path).expect("save");

        let reloaded = ContactBook::load(&path).expect("reload");
        assert_eq!(reloaded.contacts.len(), 1);
        assert_eq!(reloaded.contacts[0].name, "alice");
        assert_eq!(reloaded.contacts[0].address, pk(1));
    }

    #[test]
    fn add_rejects_duplicate_name() {
        let mut book = ContactBook::default();
        book.add(Contact {
            name: "alice".to_string(),
            address: pk(1),
        })
        .expect("first add");
        let err = book
            .add(Contact {
                name: "alice".to_string(),
                address: pk(2),
            })
            .expect_err("duplicate must fail");
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn add_rejects_pubkey_shaped_name() {
        let mut book = ContactBook::default();
        let err = book
            .add(Contact {
                name: pk(1).to_string(),
                address: pk(2),
            })
            .expect_err("pubkey-name must fail");
        assert!(err.to_string().contains("parses as a Solana address"));
    }

    #[test]
    fn remove_returns_entry_and_errors_when_missing() {
        let mut book = ContactBook::default();
        book.add(Contact {
            name: "alice".to_string(),
            address: pk(1),
        })
        .expect("add alice");
        let removed = book.remove("alice").expect("remove alice");
        assert_eq!(removed.address, pk(1));
        assert!(book.contacts.is_empty());
        let err = book.remove("alice").expect_err("second remove must fail");
        assert!(err.to_string().contains("no contact named"));
    }

    #[test]
    fn find_by_name_and_address() {
        let mut book = ContactBook::default();
        book.add(Contact {
            name: "alice".to_string(),
            address: pk(1),
        })
        .expect("add");
        assert!(book.find_by_name("alice").is_some());
        assert!(book.find_by_name("bob").is_none());
        assert!(book.find_by_address(&pk(1)).is_some());
        assert!(book.find_by_address(&pk(2)).is_none());
    }

    #[test]
    fn add_rejects_whitespace_in_name() {
        let mut book = ContactBook::default();
        let err = book
            .add(Contact {
                name: "alice bob".to_string(),
                address: pk(1),
            })
            .expect_err("whitespace name must fail");
        assert!(err.to_string().contains("whitespace"));
    }

    #[test]
    fn add_rejects_empty_name() {
        let mut book = ContactBook::default();
        let err = book
            .add(Contact {
                name: String::new(),
                address: pk(1),
            })
            .expect_err("empty name must fail");
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn resolve_with_prefers_literal_pubkey_over_same_named_contact() {
        // Security invariant: even if a malicious contacts file binds a
        // name that LOOKS like a base58 pubkey, the literal pubkey on the
        // command line must resolve to itself, never to the contact's
        // address. `add` prevents pubkey-shaped names at write time;
        // this test pins the read-time guarantee too, so a hand-edited
        // file can't override it.
        let real_addr = pk(1);
        let attacker_addr = pk(2);
        let book = ContactBook {
            contacts: vec![Contact {
                name: real_addr.to_string(),
                address: attacker_addr,
            }],
        };
        let resolved = resolve_with(&book, &real_addr.to_string()).expect("resolve literal");
        assert_eq!(resolved, real_addr);
    }

    #[test]
    fn resolve_with_falls_back_to_contact_name() {
        let mut book = ContactBook::default();
        book.add(Contact {
            name: "alice".to_string(),
            address: pk(1),
        })
        .expect("add alice");
        assert_eq!(resolve_with(&book, "alice").expect("resolve name"), pk(1));
    }

    #[test]
    fn resolve_with_errors_on_unknown_input() {
        let book = ContactBook::default();
        let err = resolve_with(&book, "nobody").expect_err("unknown must fail");
        let msg = err.to_string();
        assert!(msg.contains("not a known contact"));
        assert!(msg.contains("nobody"));
    }
}
