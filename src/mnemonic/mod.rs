use crate::result::{bail, Result};

use regex::Regex;
use sha2::{Digest, Sha256};
use structopt::{clap::arg_enum, StructOpt};

include!(concat!(env!("OUT_DIR"), "/english.rs"));

type WordList = &'static [&'static str];

arg_enum! {
    #[derive( Debug, StructOpt)]
    pub enum SeedType {
        Bip39,
        Mobile,
    }
}

pub enum Language {
    English,
}

fn get_wordlist(language: Language) -> WordList {
    match language {
        Language::English => WORDS_ENGLISH,
    }
}

/// Converts a 12 or 24 word mnemonic to entropy that can be used to
/// generate a keypair
pub fn mnemonic_to_entropy(words: Vec<String>, seed_type: &SeedType) -> Result<[u8; 32]> {
    match seed_type {
        SeedType::Bip39 => {
            if words.len() != 12 && words.len() != 24 {
                bail!(
                    "Invalid number of BIP39 seed words. Only 12 or 24 word phrases are supported."
                );
            }
        }
        SeedType::Mobile => {
            if words.len() != 12 {
                bail!(
                    "Invalid number of mobile app seed words. Only 12 word phrases are supported."
                );
            }
        }
    };

    let wordlist = get_wordlist(Language::English);

    let mut bit_vec = Vec::with_capacity(words.len());
    for word in words.iter() {
        let idx_bits = match wordlist.iter().position(|s| *s == word.to_lowercase()) {
            Some(idx) => format!("{:011b}", idx),
            _ => bail!("Seed word {} not found in wordlist", word),
        };
        bit_vec.push(idx_bits);
    }
    let bits = bit_vec.join("");

    let divider_index: usize = ((bits.len() as f64 / 33.0) * 32.0).floor() as usize;
    let (entropy_bits, checksum_bits) = bits.split_at(divider_index);

    lazy_static! {
        static ref RE_BYTES: Regex = Regex::new("(.{1,8})").unwrap();
    }

    // For up to 24 words, checksum should only ever be a single byte
    let mut checksum_bytes = [0u8; 1];
    for (idx, matched) in RE_BYTES.find_iter(&checksum_bits).enumerate() {
        checksum_bytes[idx] = binary_to_bytes(matched.as_str()) as u8;
    }

    let mut entropy_bytes = [0u8; 32];
    let valid_checksum;
    if words.len() == 12 {
        let mut entropy_base = [0u8; 16];
        for (idx, matched) in RE_BYTES.find_iter(&entropy_bits).enumerate() {
            entropy_base[idx] = binary_to_bytes(matched.as_str()) as u8;
        }

        // If this is supposed to be a BIP39 12-word phrase, verify the
        // checksum. Otherwise assume it is a phrase from the mobile app.
        // The mobile wallet does not calculate the checksum bits right so
        // its checksums are always 0
        valid_checksum = match seed_type {
            SeedType::Bip39 => calc_checksum_128(entropy_base),
            SeedType::Mobile => 0,
        };
        entropy_bytes[..16].copy_from_slice(&entropy_base);
        entropy_bytes[16..].copy_from_slice(&entropy_base);
    } else {
        for (idx, matched) in RE_BYTES.find_iter(&entropy_bits).enumerate() {
            entropy_bytes[idx] = binary_to_bytes(matched.as_str()) as u8;
        }

        // 24-word phrases can't be from the mobile wallet so it should have
        // a valid BIP39 checksum.
        valid_checksum = calc_checksum_256(entropy_bytes);
    }

    if checksum_bytes[0] != valid_checksum {
        bail!("Checksum failed. Invalid seed phrase.");
    }

    Ok(entropy_bytes)
}

fn calc_checksum_128(bytes: [u8; 16]) -> u8 {
    // For 128-bit entropy, checksum is the first four bits of the sha256 hash
    (Sha256::digest(&bytes)[0] & 0b11110000) >> 4
}

fn calc_checksum_256(bytes: [u8; 32]) -> u8 {
    // For 256-bit entropy, checksum is the first byte of the sha256 hash
    Sha256::digest(&bytes)[0]
}

/// Converts a binary string into an integer
fn binary_to_bytes(bin: &str) -> usize {
    usize::from_str_radix(bin, 2).unwrap() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_mobile_12_words() {
        // The words and entropy here were generated as follows: from the JS mobile-wallet implementation
        let words = "catch poet clog intact scare jacket throw palm illegal buyer allow figure";
        let expected_entropy = bs58::decode("3RrA1FDa6mdw5JwKbUxEbZbMcJgSyWjhNwxsbX5pSos8")
            .into_vec()
            .expect("decoded entropy");

        let word_list = words.split_whitespace().map(|w| w.to_string()).collect();
        let entropy = mnemonic_to_entropy(word_list, SeedType::Mobile).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }

    #[test]
    fn decode_bip39_12_words() {
        // The words and entropy here were generated as follows:
        // - Generate 12-words using https://iancoleman.io/bip39/. Record the words and the hex
        //   string of the entropy data: "ba8e05a43008eb85cbde771e945b53c6". Repeat this twice
        //   to expand it to 256-bit of entropy:
        //   "ba8e05a43008eb85cbde771e945b53c6ba8e05a43008eb85cbde771e945b53c6"
        // - Use https://www.appdevtools.com/base58-encoder-decoder with "Treat Input as HEX" to
        //   get the base58 encoded string of the expanded hex entropy
        let words = "ritual ice harbor gas modify seed control solve burden people stay million";
        let expected_entropy = bs58::decode("DZESLNVfmfzdkwAPEjyUXQ8cBtLDCHX13wXMA6pyP7uP")
            .into_vec()
            .expect("decoded entropy");

        let word_list = words.split_whitespace().map(|w| w.to_string()).collect();
        let entropy = mnemonic_to_entropy(word_list, SeedType::Bip39).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }

    #[test]
    fn decode_bip39_24_words() {
        // The words and entropy here were generated as follows:
        // - Generate 24-words using https://iancoleman.io/bip39/. Record the words and the hex
        //   string of the entropy data:
        //   "a25a2f741551c8df96dca228389425477e33d0b94d0b99084738263952c76678"
        // - Use https://www.appdevtools.com/base58-encoder-decoder with "Treat Input as HEX" to
        //   get the base58 encoded string.
        let words = "pelican sphere tackle click broken hurt fork nephew choice seven announce moment tobacco tribe topple pause october drama sock erase news glove okay bubble";
        let expected_entropy = bs58::decode("BvkoqCYcm8Ukcm6tsuRyovQRxMPNwvc6Ag3LR1ZfjRPm")
            .into_vec()
            .expect("decoded entropy");

        let word_list = words.split_whitespace().map(|w| w.to_string()).collect();
        let entropy = mnemonic_to_entropy(word_list, SeedType::Bip39).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }
}
