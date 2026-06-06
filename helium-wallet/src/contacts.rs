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
        // Reject names that parse as a wallet address in any of the forms
        // the CLI resolves. Otherwise a contact could shadow a real
        // address — `info <name>` would return the contact instead of
        // doing the Solana-or-helium → Pubkey conversion. The Solana
        // case covers `transfer one`, `squads`, etc; the helium case
        // covers `info`'s helium-base58 fallback path.
        if Pubkey::from_str(name).is_ok() {
            bail!("contact name '{name}' parses as a Solana address; pick a non-pubkey name");
        }
        if helium_crypto::PublicKey::from_str(name).is_ok() {
            bail!("contact name '{name}' parses as a Helium pubkey; pick a non-pubkey name");
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
/// use; a missing file degrades to an empty book so name resolution
/// can never break commands that don't use the feature. A *malformed*
/// file also degrades to empty, but emits a warning on stderr —
/// otherwise a subsequent `contacts add` would atomically overwrite
/// the broken (but recoverable) file with a fresh one containing only
/// the new entry, destroying every other contact.
pub fn cached() -> &'static ContactBook {
    static BOOK: OnceLock<ContactBook> = OnceLock::new();
    BOOK.get_or_init(|| {
        let path = match ContactBook::default_path() {
            Ok(p) => p,
            Err(err) => {
                eprintln!("warning: failed to resolve contacts file path: {err}");
                return ContactBook::default();
            }
        };
        match ContactBook::load(&path) {
            Ok(book) => book,
            Err(err) => {
                eprintln!(
                    "warning: failed to load contacts from {}: {err}",
                    path.display()
                );
                eprintln!(
                    "         continuing with an empty address book. \
                     fix the file before running `contacts add` — \
                     it will atomically overwrite the existing file."
                );
                ContactBook::default()
            }
        }
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

/// Serde variant of `parse_address_or_name` for fields read from JSON
/// input files (e.g. `transfer multi`'s payee list). Lets a contact
/// name appear inside the JSON wherever a base58 pubkey is accepted,
/// matching the CLI-arg behavior.
pub mod serde_address_or_name {
    use super::*;
    use serde::de::{self, Deserialize};

    pub fn serialize<S: serde::Serializer>(
        value: &Pubkey,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serde_pubkey::serialize(value, serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Pubkey, D::Error> {
        let input = String::deserialize(deserializer)?;
        resolve_with(cached(), &input).map_err(de::Error::custom)
    }
}

/// JSON keys that identify the *subject* of an object — the pubkey
/// this object IS. A matching contact attaches as the sibling key
/// `name`. Used by the JSON walker; see `enrich_pubkeys_in_place`.
const IDENTITY_KEYS: &[&str] = &["pubkey", "key", "address"];

/// JSON keys that name a related entity — the pubkey this object
/// HAS-A. A matching contact attaches as the sibling key
/// `<original>_name` (e.g. `multisig` → `multisig_name`). Kept
/// narrow so unrelated string fields can't accidentally trigger a
/// lookup.
const RELATION_KEYS: &[&str] = &[
    "multisig",
    "vault",
    "authority",
    "creator",
    "resolved_from_vault",
];

/// Walk `value` and annotate every recognized pubkey-bearing field
/// with the contact name registered for it, when one exists. Two
/// patterns are recognized — see `IDENTITY_KEYS` / `RELATION_KEYS`.
///
/// Purely additive: an object with no matching contacts is unchanged.
/// An object that already has a `name` field is never overwritten —
/// callers can pre-populate names for entities outside the contacts
/// book.
pub fn enrich_pubkeys_in_place(value: &mut serde_json::Value) {
    enrich(value, cached());
}

fn enrich(value: &mut serde_json::Value, book: &ContactBook) {
    match value {
        serde_json::Value::Object(map) => {
            // Relation pass: for each `<key>` in RELATION_KEYS, attach
            // a `<key>_name` sibling only when the sibling isn't
            // already set — symmetric with the identity-key guard
            // below, so a caller pre-populating `vault_name` (say, to
            // label a non-contact entity) isn't clobbered.
            let relation_inserts: Vec<(String, String)> = RELATION_KEYS
                .iter()
                .filter_map(|key| {
                    let sibling = format!("{key}_name");
                    if map.contains_key(&sibling) {
                        return None;
                    }
                    let addr = map.get(*key)?.as_str()?;
                    let pk = Pubkey::from_str(addr).ok()?;
                    let name = book.find_by_address(&pk)?.name.clone();
                    Some((sibling, name))
                })
                .collect();
            for (k, v) in relation_inserts {
                map.insert(k, serde_json::Value::String(v));
            }

            if !map.contains_key("name") {
                let identity_name = IDENTITY_KEYS.iter().find_map(|key| {
                    let addr = map.get(*key)?.as_str()?;
                    let pk = Pubkey::from_str(addr).ok()?;
                    book.find_by_address(&pk).map(|c| c.name.clone())
                });
                if let Some(name) = identity_name {
                    map.insert("name".to_string(), serde_json::Value::String(name));
                }
            }

            for v in map.values_mut() {
                enrich(v, book);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                enrich(v, book);
            }
        }
        _ => {}
    }
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
    fn add_rejects_helium_pubkey_shaped_name() {
        // info.rs's parse_address falls back to helium_crypto::PublicKey
        // after the contact lookup. If a helium-shaped name were
        // allowed, a contact could shadow the helium → solana
        // conversion — same class of bug as the Solana case above.
        // Round-trip a known Solana pubkey through the helium format
        // to get a name string the helium parser accepts.
        let helium = helium_lib::keypair::to_helium_pubkey(&pk(1))
            .expect("convert test pubkey to helium format");
        let helium_name = helium.to_string();
        helium_crypto::PublicKey::from_str(&helium_name)
            .expect("round-tripped string parses as helium pubkey");

        let mut book = ContactBook::default();
        let err = book
            .add(Contact {
                name: helium_name,
                address: pk(2),
            })
            .expect_err("helium-pubkey-shaped name must fail");
        assert!(err.to_string().contains("Helium pubkey"));
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

    fn book_with(entries: &[(&str, Pubkey)]) -> ContactBook {
        let mut book = ContactBook::default();
        for (name, address) in entries {
            book.add(Contact {
                name: (*name).to_string(),
                address: *address,
            })
            .expect("add");
        }
        book
    }

    #[test]
    fn walker_injects_name_next_to_identity_keys() {
        let book = book_with(&[("alice", pk(1))]);
        let cases = ["pubkey", "key", "address"];
        for field in cases {
            let mut value = serde_json::json!({ field: pk(1).to_string() });
            super::enrich(&mut value, &book);
            assert_eq!(value[field].as_str(), Some(pk(1).to_string().as_str()));
            assert_eq!(value["name"].as_str(), Some("alice"), "field = {field}");
        }
    }

    #[test]
    fn walker_injects_suffixed_name_for_relation_keys() {
        let book = book_with(&[
            ("treasury", pk(1)),
            ("alice", pk(2)),
            ("vault-prod", pk(3)),
            ("ops-vault", pk(4)),
        ]);
        let mut value = serde_json::json!({
            "multisig": pk(1).to_string(),
            "creator": pk(2).to_string(),
            "vault": pk(3).to_string(),
            "authority": pk(4).to_string(),
        });
        super::enrich(&mut value, &book);
        assert_eq!(value["multisig_name"].as_str(), Some("treasury"));
        assert_eq!(value["creator_name"].as_str(), Some("alice"));
        assert_eq!(value["vault_name"].as_str(), Some("vault-prod"));
        assert_eq!(value["authority_name"].as_str(), Some("ops-vault"));
    }

    #[test]
    fn walker_recurses_into_arrays_and_nested_objects() {
        let book = book_with(&[("alice", pk(1)), ("bob", pk(2))]);
        let mut value = serde_json::json!({
            "instructions": [
                {
                    "accounts": [
                        { "pubkey": pk(1).to_string(), "writable": true },
                        { "pubkey": pk(2).to_string(), "writable": false },
                        { "pubkey": pk(3).to_string(), "writable": false },
                    ]
                }
            ]
        });
        super::enrich(&mut value, &book);
        let accounts = &value["instructions"][0]["accounts"];
        assert_eq!(accounts[0]["name"].as_str(), Some("alice"));
        assert_eq!(accounts[1]["name"].as_str(), Some("bob"));
        assert!(
            accounts[2].get("name").is_none(),
            "unknown pubkey gets no name"
        );
    }

    #[test]
    fn walker_skips_unknown_addresses_silently() {
        let book = book_with(&[("alice", pk(1))]);
        let original = serde_json::json!({
            "multisig": pk(99).to_string(),
            "pubkey": pk(99).to_string(),
        });
        let mut value = original.clone();
        super::enrich(&mut value, &book);
        assert_eq!(value, original, "unknown addresses must not be annotated");
    }

    #[test]
    fn walker_preserves_existing_name_field() {
        let book = book_with(&[("alice", pk(1))]);
        // Caller pre-populated `name` for a token / program / etc;
        // the walker must not overwrite it with the contact name.
        let mut value = serde_json::json!({
            "pubkey": pk(1).to_string(),
            "name": "HNT mint",
        });
        super::enrich(&mut value, &book);
        assert_eq!(value["name"].as_str(), Some("HNT mint"));
    }

    #[test]
    fn walker_preserves_existing_relation_name_field() {
        // Symmetric guard with the identity-key case above: a caller
        // that pre-populated `multisig_name` (e.g. to label a non-
        // contact entity with a specific reviewer-facing label) keeps
        // it; the walker doesn't clobber.
        let book = book_with(&[("treasury", pk(1))]);
        let mut value = serde_json::json!({
            "multisig": pk(1).to_string(),
            "multisig_name": "operator-set-2025-q2",
        });
        super::enrich(&mut value, &book);
        assert_eq!(
            value["multisig_name"].as_str(),
            Some("operator-set-2025-q2")
        );
    }

    #[test]
    fn walker_enriches_resolved_from_vault() {
        // MultisigInfo carries `resolved_from_vault` when the input
        // target was a vault PDA. It should be named in the same way
        // `multisig` and `vault` are.
        let book = book_with(&[("ops-vault", pk(1))]);
        let mut value = serde_json::json!({
            "resolved_from_vault": pk(1).to_string(),
        });
        super::enrich(&mut value, &book);
        assert_eq!(
            value["resolved_from_vault_name"].as_str(),
            Some("ops-vault")
        );
    }

    #[test]
    fn walker_ignores_non_pubkey_strings_in_known_keys() {
        let book = book_with(&[("alice", pk(1))]);
        // A field literally named "address" can hold a non-pubkey
        // string (e.g. a hotspot's location string). The walker must
        // not trip over it.
        let mut value = serde_json::json!({
            "address": "123 Main St",
        });
        super::enrich(&mut value, &book);
        assert!(value.get("name").is_none());
    }
}
