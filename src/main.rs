//! The git-pincer executable entry: Parses arguments and dispatches them to subcommands.

use clap::Parser;
use git_pincer::cli::Cli;

fn main() {
    if let Err(e) = Cli::parse().run() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
