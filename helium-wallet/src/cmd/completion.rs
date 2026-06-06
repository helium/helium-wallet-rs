use crate::result::Result;
use clap_complete::Shell;

/// Generate a shell completion script for the given shell
#[derive(Debug, clap::Args)]
pub struct Cmd {
    /// Target shell to generate completion for
    #[arg(value_enum)]
    shell: Shell,
}

impl Cmd {
    pub fn run<C: clap::CommandFactory>(&self) -> Result {
        let mut cmd = C::command();
        let bin = cmd.get_name().to_string();
        clap_complete::generate(self.shell, &mut cmd, bin, &mut std::io::stdout());
        Ok(())
    }
}
