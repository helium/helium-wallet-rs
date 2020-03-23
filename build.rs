use std::env;
use std::fs::{self, DirEntry, File};
// use std::io::Write;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::Path,
    result,
};

type Result<T> = result::Result<T, io::Error>;

fn read_wordlist(dir: Result<DirEntry>) -> Result<(OsString, Vec<String>)> {
    let path = dir?.path();
    let buf = fs::read_to_string(path.clone())?;
    let words: Vec<String> = buf.split_whitespace().map(|w| w.to_string()).collect();
    Ok((path.file_stem().unwrap().to_os_string(), words))
}

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let wordlist_paths = fs::read_dir(Path::new("src/mnemonic/wordlists")).unwrap();
    for path in wordlist_paths {
        let (lang, words) = read_wordlist(path).unwrap();
        let dest_path = Path::new(&out_dir).join(lang.clone()).with_extension(&"rs");
        let mut dest_file = File::create(&dest_path).unwrap();
        writeln!(
            dest_file,
            "pub const WORDS_{}: WordList = &[",
            lang.to_str().unwrap().to_uppercase()
        )
        .unwrap();
        for word in words {
            writeln!(dest_file, "\"{}\",", word).unwrap();
        }
        write!(dest_file, "];").unwrap();
    }
}
