use helium_wallet::wallet::Wallet;

fn main() {
    let password = "pass123";
    let filename = "my-example-wallet.key";

    let wallet = Wallet::builder()
        .password(password)
        .output(&filename.into())
        .force(true)
        .create()
        .expect("it should have created a wallet");

    let keypair = wallet
        .decrypt(password.as_bytes())
        .expect("it should decrypt the wallet");

    println!("{:?}", keypair);
}
