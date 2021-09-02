use helium_wallet::wallet::Wallet;
use std::path::Path;

fn main() {
    let password = "pass123";
    let filename = Path::new("my-example-wallet.key");

    let wallet = Wallet::builder()
        .password(password)
        .output(&filename)
        .force(true)
        .create()
        .expect("it should have created a wallet");

    let keypair = wallet
        .decrypt(password.as_bytes())
        .expect("it should decrypt the wallet");

    println!("{:?}", keypair);
}
