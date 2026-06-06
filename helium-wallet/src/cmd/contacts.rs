use crate::{
    cmd::{print_json, Opts},
    contacts::{Contact, ContactBook},
    result::Result,
};
use helium_lib::keypair::Pubkey;

/// Manage the address book of named Solana addresses.
///
/// Contacts live at `$XDG_CONFIG_HOME/helium-wallet/contacts.json`
/// (overridable via `HELIUM_WALLET_CONTACTS`). Once a contact exists,
/// its name can be used wherever an address is accepted on the command
/// line — `transfer one --payee alice 1 hnt`, `hotspots transfer
/// <key> alice`, etc. A literal base58 pubkey always parses as itself,
/// so a contact named `7xK...` can never shadow a real address.
#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: SubCmd,
}

impl Cmd {
    pub async fn run(&self, _opts: Opts) -> Result {
        let path = ContactBook::default_path()?;
        match &self.cmd {
            SubCmd::List(c) => c.run(&path),
            SubCmd::Show(c) => c.run(&path),
            SubCmd::Add(c) => c.run(&path),
            SubCmd::Remove(c) => c.run(&path),
        }
    }
}

#[derive(Debug, clap::Subcommand)]
pub enum SubCmd {
    List(ListCmd),
    Show(ShowCmd),
    Add(AddCmd),
    Remove(RemoveCmd),
}

/// List every contact in the book.
#[derive(Debug, clap::Args)]
pub struct ListCmd;

impl ListCmd {
    fn run(&self, path: &std::path::Path) -> Result {
        let book = ContactBook::load(path)?;
        print_json(&book)
    }
}

/// Show a single contact by name.
#[derive(Debug, clap::Args)]
pub struct ShowCmd {
    name: String,
}

impl ShowCmd {
    fn run(&self, path: &std::path::Path) -> Result {
        let book = ContactBook::load(path)?;
        let contact = book
            .find_by_name(&self.name)
            .ok_or_else(|| crate::result::anyhow!("no contact named '{}'", self.name))?;
        print_json(contact)
    }
}

/// Add a new contact. `<address>` must be a literal Solana base58
/// pubkey — names never resolve to other names.
#[derive(Debug, clap::Args)]
pub struct AddCmd {
    name: String,
    address: Pubkey,
}

impl AddCmd {
    fn run(&self, path: &std::path::Path) -> Result {
        let mut book = ContactBook::load(path)?;
        let contact = Contact {
            name: self.name.clone(),
            address: self.address,
        };
        book.add(contact.clone())?;
        book.save(path)?;
        print_json(&contact)
    }
}

/// Remove a contact by name. Prints the removed entry.
#[derive(Debug, clap::Args)]
pub struct RemoveCmd {
    name: String,
}

impl RemoveCmd {
    fn run(&self, path: &std::path::Path) -> Result {
        let mut book = ContactBook::load(path)?;
        let removed = book.remove(&self.name)?;
        book.save(path)?;
        print_json(&removed)
    }
}
