use bitvec::prelude::*;
use lazy_static::lazy_static;
use sha2::{Digest, Sha256};
use std::ops::Index;

#[derive(Debug, thiserror::Error)]
pub enum MnmemonicError {
    #[error("no such word: {0}")]
    NoSuchWord(String),
    #[error("invalid word count: {0}, only 12 or 24 words supported")]
    InvalidWordCount(usize),
    #[error("Checksum failed. Invalid seed phrase")]
    InvalidChecksum,
    #[error("Incorrect entropy length: {0}")]
    InvalidEntropyLen(usize),
}

lazy_static! {
    static ref WORDS_ENGLISH: Vec<&'static str> = include_str!("english.txt").lines().collect();
}

pub enum Language {
    English,
}

impl Language {
    pub fn find_word(&self, user_word: &str) -> Option<usize> {
        match self {
            Language::English => Self::find_english_word(user_word),
        }
    }

    fn find_english_word(user_word: &str) -> Option<usize> {
        // BIP39: the wordlist is created in such a way that it's
        //        enough to type the first four letters to
        //        unambiguously identify the word
        const MIN_CMP_LEN: usize = 4;
        let user_word = user_word.to_ascii_lowercase();
        WORDS_ENGLISH.iter().position(|&bip39_word| {
            user_word == bip39_word
                || (user_word.len() >= MIN_CMP_LEN
                    && bip39_word.len() >= user_word.len()
                    && user_word == bip39_word[..user_word.len()])
        })
    }
}

impl Index<usize> for Language {
    type Output = str;
    fn index(&self, index: usize) -> &str {
        WORDS_ENGLISH[index]
    }
}

/// Converts a 12 or 24 word mnemonic to entropy that can be used to
/// generate a keypair
pub fn mnemonic_to_entropy(words: Vec<String>) -> Result<[u8; 32], MnmemonicError> {
    const MAX_ENTROPY_BITS: usize = 256;
    const BITS_PER_WORD: usize = 11;
    const CHECKSUM_BITS_PER_WORD: usize = 3;

    let words_len = words.len();
    if words_len != 12 && words_len != 24 {
        return Err(MnmemonicError::InvalidWordCount(words_len));
    }

    // Build of word_bits in an accumulator by iterating thru the word vector, looking
    // up the index of each word. For each index, copy only the least-significant
    // BITS_PER_WORD bits onto the end of the accumulator.
    let language = Language::English;
    let word_bits =
        words
            .into_iter()
            .try_fold(BitVec::with_capacity(MAX_ENTROPY_BITS), |mut acc, w| {
                language
                    .find_word(&w)
                    .ok_or(MnmemonicError::NoSuchWord(w))
                    .map(|idx| {
                        let idx_bits = &idx.view_bits::<Msb0>();
                        acc.extend_from_bitslice(&idx_bits[idx_bits.len() - BITS_PER_WORD..]);
                        acc
                    })
            })?;

    let divider_index: usize = word_bits.len() - (words_len / CHECKSUM_BITS_PER_WORD);
    let (entropy_bits, checksum_bits) = word_bits.split_at(divider_index);
    // For up to 24 words, checksum should only ever be a single byte
    let checksum_byte: u8 = checksum_bits.load::<u8>();

    let mut entropy_bytes = [0u8; 32];
    let valid_checksum = if words_len == 12 {
        // Duplicate entropy bits into the first half and last half of the final
        // byte array so we can always return 256 bits (32 bytes) of entropy.
        // Keep entropy_half instead of doing this inline so we can calculate the
        // checksum.
        let mut entropy_half = [0u8; 16];
        entropy_half
            .view_bits_mut::<Msb0>()
            .copy_from_bitslice(entropy_bits);
        entropy_bytes[..16].copy_from_slice(&entropy_half);
        entropy_bytes[16..].copy_from_slice(&entropy_half);

        calc_checksum_128(entropy_half)
    } else {
        entropy_bytes
            .view_bits_mut::<Msb0>()
            .copy_from_bitslice(entropy_bits);

        calc_checksum_256(entropy_bytes)
    };

    // Some wallet apps have produced phrases with a checksum of '0'
    // so allow those for backwards compatibility.
    if (checksum_byte != valid_checksum) && (checksum_byte != 0) {
        return Err(MnmemonicError::InvalidChecksum);
    }

    Ok(entropy_bytes)
}

/// Given some entropy of the proper length, return a mnemonic phrase.
/// Inspired by the bip39 crate. https://docs.rs/bip39/1.0.1/bip39/index.html
pub fn entropy_to_mnemonic(entropy: &[u8]) -> Result<Vec<String>, MnmemonicError> {
    const MAX_ENTROPY_BITS: usize = 256;
    const MIN_ENTROPY_BITS: usize = 128;
    const ENTROPY_MULTIPLE: usize = 32;
    const BITS_PER_WORD: usize = 11;

    let midpoint = entropy.len() / 2;
    let (front, back) = entropy.split_at(midpoint);
    let working_entropy = if front == back { front } else { entropy };
    let working_bits = working_entropy.len() * 8;

    if working_bits % ENTROPY_MULTIPLE != 0
        || !(MIN_ENTROPY_BITS..=MAX_ENTROPY_BITS).contains(&working_bits)
    {
        return Err(MnmemonicError::InvalidEntropyLen(entropy.len()));
    }

    let mut word_bits = BitVec::with_capacity(MAX_ENTROPY_BITS);
    word_bits.extend_from_bitslice(working_entropy.view_bits::<Msb0>());

    // For every 32-bits of entropy, add one bit of checksum to
    // the end of word_bits.
    let checksum = Sha256::digest(working_entropy);

    let check_bits = checksum.view_bits::<Msb0>();
    word_bits.extend_from_bitslice(&check_bits[..working_bits / ENTROPY_MULTIPLE]);
    let mut words = Vec::with_capacity(word_bits.len() / BITS_PER_WORD);

    // For every group of 11 bits, use the value as an index into the word list.
    // Then push the resulting word string into our words vector.
    let language = Language::English;
    for c in word_bits.chunks(BITS_PER_WORD) {
        let mut idx: usize = 0;
        let idx_bits = idx.view_bits_mut::<Msb0>();
        let len = idx_bits.len() - BITS_PER_WORD;
        idx_bits[len..].copy_from_bitslice(c);
        words.push(language[idx].to_string());
    }

    Ok(words)
}

fn calc_checksum_128(bytes: [u8; 16]) -> u8 {
    // For 128-bit entropy, checksum is the first four bits of the sha256 hash
    (Sha256::digest(bytes)[0] & 0b11110000) >> 4
}

fn calc_checksum_256(bytes: [u8; 32]) -> u8 {
    // For 256-bit entropy, checksum is the first byte of the sha256 hash
    Sha256::digest(bytes)[0]
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
        let entropy = mnemonic_to_entropy(word_list).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }

    #[test]
    fn decode_tuncated_mobile_12_words() {
        // The words and entropy here were generated as follows: from the JS mobile-wallet implementation
        let words = "catc poet clog inta scar jack throw palm ille buye allo figu";
        let expected_entropy = bs58::decode("3RrA1FDa6mdw5JwKbUxEbZbMcJgSyWjhNwxsbX5pSos8")
            .into_vec()
            .expect("decoded entropy");

        let word_list = words.split_whitespace().map(|w| w.to_string()).collect();
        let entropy = mnemonic_to_entropy(word_list).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }

    #[test]
    fn encode_mobile_12_words() {
        // This test starts with zero-checksum 12 word phrase from the helium-hotspot-app, turns it into
        // entropy, then decodes that entropy back into a 12 word phrase with a proper checksum.
        let hotspot_app_word_list =
            "catch poet clog intact scare jacket throw palm illegal buyer allow figure"
                .split_whitespace()
                .map(|w| w.to_string())
                .collect();
        let bip39_words_list: Vec<String> =
            "catch poet clog intact scare jacket throw palm illegal buyer allow firm"
                .split_whitespace()
                .map(|w| w.to_string())
                .collect();

        let hotspot_app_entropy =
            mnemonic_to_entropy(hotspot_app_word_list).expect("hotspot_app_entropy");
        let bip39_entropy = mnemonic_to_entropy(bip39_words_list.to_vec()).expect("bip39_entropy");

        let expected_entropy = bs58::decode("3RrA1FDa6mdw5JwKbUxEbZbMcJgSyWjhNwxsbX5pSos8")
            .into_vec()
            .expect("expected_entropy");

        assert_eq!(expected_entropy, hotspot_app_entropy);
        assert_eq!(expected_entropy, bip39_entropy);

        let decoded_hotspot_word_list =
            entropy_to_mnemonic(&hotspot_app_entropy).expect("decoded_hotspot_words");

        assert_eq!(bip39_words_list, decoded_hotspot_word_list);
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
        let entropy = mnemonic_to_entropy(word_list).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }

    #[test]
    fn decode_truncated_bip39_12_words() {
        // The words and entropy here were generated as follows:
        // - Generate 12-words using https://iancoleman.io/bip39/. Record the words and the hex
        //   string of the entropy data: "ba8e05a43008eb85cbde771e945b53c6". Repeat this twice
        //   to expand it to 256-bit of entropy:
        //   "ba8e05a43008eb85cbde771e945b53c6ba8e05a43008eb85cbde771e945b53c6"
        // - Use https://www.appdevtools.com/base58-encoder-decoder with "Treat Input as HEX" to
        //   get the base58 encoded string of the expanded hex entropy
        let words = "ritu ice harb gas modi seed contr solv burd people stay millio";
        let expected_entropy = bs58::decode("DZESLNVfmfzdkwAPEjyUXQ8cBtLDCHX13wXMA6pyP7uP")
            .into_vec()
            .expect("decoded entropy");

        let word_list = words.split_whitespace().map(|w| w.to_string()).collect();
        let entropy = mnemonic_to_entropy(word_list).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }

    #[test]
    fn encode_bip39_12_words() {
        // The words and entropy here were generated as follows: from the JS mobile-wallet implementation
        let entropy = bs58::decode("DZESLNVfmfzdkwAPEjyUXQ8cBtLDCHX13wXMA6pyP7uP")
            .into_vec()
            .expect("decoded entropy");

        let expected_words =
            "ritual ice harbor gas modify seed control solve burden people stay million";
        let words = entropy_to_mnemonic(&entropy).expect("mnemonic").join(" ");
        assert_eq!(expected_words, words);
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
        let entropy = mnemonic_to_entropy(word_list).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }

    #[test]
    fn decode_tuncated_bip39_24_words() {
        // The words and entropy here were generated as follows:
        // - Generate 24-words using https://iancoleman.io/bip39/. Record the words and the hex
        //   string of the entropy data:
        //   "a25a2f741551c8df96dca228389425477e33d0b94d0b99084738263952c76678"
        // - Use https://www.appdevtools.com/base58-encoder-decoder with "Treat Input as HEX" to
        //   get the base58 encoded string.
        let words = "peli sphe tack click brok hurt fork neph choic seve anno mome toba trib topp paus octo dram sock eras news glov okay bubb";
        let expected_entropy = bs58::decode("BvkoqCYcm8Ukcm6tsuRyovQRxMPNwvc6Ag3LR1ZfjRPm")
            .into_vec()
            .expect("decoded entropy");

        let word_list = words.split_whitespace().map(|w| w.to_string()).collect();
        let entropy = mnemonic_to_entropy(word_list).expect("entropy");
        assert_eq!(expected_entropy, entropy);
    }

    #[test]
    fn encode_bip39_24_words() {
        // The words and entropy here were generated as follows: from the JS mobile-wallet implementation
        let entropy = bs58::decode("BvkoqCYcm8Ukcm6tsuRyovQRxMPNwvc6Ag3LR1ZfjRPm")
            .into_vec()
            .expect("decoded entropy");

        let expected_words = "pelican sphere tackle click broken hurt fork nephew choice seven announce moment tobacco tribe topple pause october drama sock erase news glove okay bubble";
        let words = entropy_to_mnemonic(&entropy).expect("mnemonic").join(" ");
        assert_eq!(expected_words, words);
    }
}
