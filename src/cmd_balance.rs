use crate::{
    result::Result
};
use helium_api::Client;

pub fn cmd_balance(addresses: Vec<String>) -> Result {
    let client = Client::new();
    for address in addresses {
        print_account(&client, &address);
    }

    Ok(())
}

fn print_account(client: &Client, address: &str)  {
    let account = match client.get_account(address) {
        Ok(a) => a,
        Err(err) => {
            println!("Address: {}", address);
            println!("Error: {:?}", err);
            return
        }
    };
    println!("Address: {}", address);
    println!("Balance: {}", account.balance);
    println!("Data Credits: {}", account.dc_balance);
    println!("Security Balance: {}", account.security_balance);
    println!();
}
