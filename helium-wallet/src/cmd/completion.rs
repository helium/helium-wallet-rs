use clap_complete::Shell;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    /// Target shell (bash, zsh, fish, powershell, elvish).
    #[arg(value_enum)]
    pub shell: Shell,
}
