//! The git-pincer executable entry: Parses arguments and dispatches them to subcommands.

use anyhow::Result;
use clap::Parser;
use git_pincer::cli::Cli;

fn main() -> Result<()> {
    Cli::parse().run()
}
