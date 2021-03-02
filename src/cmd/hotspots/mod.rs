use crate::{cmd::*, result::Result};
use structopt::StructOpt;

mod add;
mod assert;
mod list;
mod transfer;

#[derive(Debug, StructOpt)]
/// Display list of hotspots associated with wallet
/// or transfer a hotspot to another wallet
pub enum Cmd {
    Add(add::Cmd),
    Assert(assert::Cmd),
    List(list::Cmd),
    Transfer(Box<transfer::Cmd>),
}

impl Cmd {
    pub fn run(self, opts: Opts) -> Result {
        match self {
            Self::Add(cmd) => cmd.run(opts),
            Self::Assert(cmd) => cmd.run(opts),
            Self::List(cmd) => cmd.run(opts),
            Self::Transfer(cmd) => cmd.run(opts),
        }
    }
}
