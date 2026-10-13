//! completions subcommand: generate shell completion scripts to stdout.

use clap::CommandFactory;
use clap_complete::Shell;

/// Arguments for the completions subcommand.
#[derive(Debug, clap::Args)]
pub struct CompletionsArgs {
    /// Target shell (bash / zsh / fish / powershell / elvish)
    #[arg(value_enum)]
    pub shell: Shell,
}

/// Write a completion script to stdout for sourcing during shell initialization.
pub fn run(shell: Shell) {
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "git-pincer", &mut std::io::stdout());
}
