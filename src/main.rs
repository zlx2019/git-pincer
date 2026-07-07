//! The git-peace executable entry: Parses arguments and dispatches them to subcommands.

use anyhow::Result;
use clap::Parser;
use git_peace::cli::Cli;

fn main() -> Result<()> {
    Cli::parse().run()
}
