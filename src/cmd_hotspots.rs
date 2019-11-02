use crate::result::Result;
use helium_api::Client;

pub fn cmd_hotspots(addresses: Vec<String>) -> Result {
    let client = Client::new();
    for address in addresses {
        print_account(&client, &address);
    }

    Ok(())
}

fn print_account(client: &Client, address: &str) {
    let hotspots = match client.get_hotspots(address) {
        Ok(a) => a,
        Err(err) => {
            println!("Address: {}", address);
            println!("Error: {:?}", err);
            return;
        }
    };
    println!("Hotspotted: {:?}", hotspots);
    println!();
}
