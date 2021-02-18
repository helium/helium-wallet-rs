use crate::result::{bail, Result};
use regex::Regex;

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
        bail!("Invalid number of seed words");
    }
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
    // The mobile wallet does not calculate the checksum bits right so
    // they always and up being all 0
    if checksum_bits != "0000" {
        bail!("invalid checksum");
    }

    lazy_static! {
        static ref RE_BYTES: Regex = Regex::new("(.{1,8})").unwrap();
    }

    let mut entropy_base = [0u8; 16];
    for (idx, matched) in RE_BYTES.find_iter(&entropy_bits).enumerate() {
        entropy_base[idx] = binary_to_bytes(matched.as_str()) as u8;
    }

    let mut entropy_bytes = [0u8; 32];
    entropy_bytes[..16].copy_from_slice(&entropy_base);
    entropy_bytes[16..].copy_from_slice(&entropy_base);

    Ok(entropy_bytes)
}

/// Converts a binary string into an integer
fn binary_to_bytes(bin: &str) -> usize {
    usize::from_str_radix(bin, 2).unwrap() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_words() {
        // The words and entryopy here were generated from the JS mobile-wallet implementation
        let words = "catch poet clog intact scare jacket throw palm illegal buyer allow figure";
        let expected_entropy = bs58::decode("3RrA1FDa6mdw5JwKbUxEbZbMcJgSyWjhNwxsbX5pSos8")
            .into_vec()
            .expect("decoded entropy");

        let word_list = words.split_whitespace().map(|w| w.to_string()).collect();
        let entropy = mnemonic_to_entropy(word_list).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }
}
