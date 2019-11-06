use crate::result::Result;
use regex::Regex;
use sha2::{Digest, Sha256};
include!(concat!(env!("OUT_DIR"), "/english.rs"));

type WordList = &'static [&'static str];

pub enum Language {
    English,
}

fn get_wordlist(language: Language) -> WordList {
    match language {
        Language::English => WORDS_ENGLISH,
    }
}

/// Converts a 12 word mnemonic to a entropy that can be used to
/// generate a keypair
pub fn mnemonic_to_entropy(words: Vec<String>) -> Result<[u8; 32]> {
    if words.len() != 12 {
        return Err("Invalid number of seed words".into());
    }
    let wordlist = get_wordlist(Language::English);

    let mut bit_vec = Vec::with_capacity(words.len());
    for word in words.iter() {
        let idx_bits = match wordlist.iter().position(|s| *s == word) {
            Some(idx) => format!("{:011b}", idx),
            _ => return Err(format!("Seed word {} not found in wordlist", word).into()),
        };
        bit_vec.push(idx_bits);
    }
    let bits = bit_vec.join("");

    let divider_index: usize = ((bits.len() as f64 / 33.0) * 32.0).floor() as usize;
    let (entropy_bits, checksum_bits) = bits.split_at(divider_index);

    lazy_static! {
        static ref RE_BYTES: Regex = Regex::new("(.{1,8})").unwrap();
    }

    let mut entropy_base = [0u8; 16];
    for (idx, matched) in RE_BYTES.find_iter(&entropy_bits).enumerate() {
        entropy_base[idx] = binary_to_bytes(matched.as_str()) as u8;
    }

    let new_checksum = derive_checksum_bits(&entropy_base);
    assert!(checksum_bits == new_checksum, "invalid checksum");

    let mut entropy_bytes = [0u8; 32];
    entropy_bytes[..16].copy_from_slice(&entropy_base);
    entropy_bytes[16..].copy_from_slice(&entropy_base);

    Ok(entropy_bytes)
}

/// Converts a vec of bytes into a single binary number string.
fn bytes_to_binary(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:08b}", b))
        .collect::<Vec<String>>()
        .join("")
}

/// Converts a binary string into an integer
fn binary_to_bytes(bin: &str) -> usize {
    usize::from_str_radix(bin, 2).unwrap() as usize
}

/// Calculates checksum bits for entropy and returns
/// a single binary number string.
fn derive_checksum_bits(entropy: &[u8; 16]) -> String {
    let ent = entropy.len() * 8;
    let cs = ent / 32;

    let mut hasher = Sha256::new();
    hasher.input(entropy);
    let hash = hasher.result();

    bytes_to_binary(&hash.as_slice().to_vec())[..cs].to_string()
}
