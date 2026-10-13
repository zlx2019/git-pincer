//! CLI definition and subcommand dispatch.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::commands;

/// Program command line
#[derive(Debug, Parser)]
#[command(
    version,
    about,
    long_about = None
)]
pub struct Cli {
    /// The specific subcommand to be executed
    #[command(subcommand)]
    pub command: Option<Commands>,
    /// The Git repository path (default is current directory)
    #[arg(short = 'C', long = "repo", global = true, value_name = "PATH")]
    pub repo: Option<PathBuf>,
    /// Echo executed git command
    #[arg(short, long, global = true)]
    pub verbose: bool,
    /// theme
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub theme: ThemeArg,
    /// UI language
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub lang: LangArg,
}

/// UI language selection shared by the CLI and `[ui] lang` configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LangArg {
    /// Detect from the system locale (Chinese for a zh prefix, English otherwise)
    Auto,
    /// Chinese
    Zh,
    /// English
    En,
}

/// UI theme selection shared by the CLI and `[ui] theme` configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeArg {
    /// Detect from COLORFGBG, falling back to the dark theme
    Auto,
    /// Dark (Tokyo Night)
    Dark,
    /// Light (Maple Light, for light terminal backgrounds)
    Light,
}

/// Supported subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run `git merge` and take over conflicts if any
    Merge(commands::run::MergeArgs),
    /// Run `git rebase`, looping through every conflicted round
    Rebase(commands::run::RebaseArgs),
    /// Run `git pull` (all arguments are passed through)
    Pull(commands::run::PullArgs),
    /// Run `git cherry-pick` (all arguments are passed through)
    CherryPick(commands::run::CherryPickArgs),
    /// Run `git revert` (all arguments are passed through)
    Revert(commands::run::RevertArgs),
    /// Switch to another branch (remote branches get a local tracking branch)
    Switch(commands::switch::SwitchArgs),
    /// Resolve a single conflict-marked file, no git required
    File(commands::file::FileArgs),
    /// Abort the operation in progress (merge / rebase / cherry-pick / revert / am)
    Abort,
    /// Generate a shell completion script to stdout
    Completions(commands::completions::CompletionsArgs),
}

impl Cli {
    /// Distribute and execute the selected subcommands.
    pub fn run(self) -> Result<()> {
        // Generate completions before loading configuration: shell startup must
        // not be blocked by configuration errors or Git environment issues.
        if let Some(Commands::Completions(args)) = &self.command {
            commands::completions::run(args.shell);
            return Ok(());
        }
        // Apply an explicit --lang early so configuration errors use that language.
        // The first i18n::init wins: CLI > configuration > system detection.
        match self.lang {
            LangArg::Zh => crate::i18n::init(crate::i18n::Lang::Zh),
            LangArg::En => crate::i18n::init(crate::i18n::Lang::En),
            LangArg::Auto => {}
        }
        let config = crate::config::load()?;
        crate::i18n::init(match config.ui.lang {
            Some(LangArg::Zh) => crate::i18n::Lang::Zh,
            Some(LangArg::En) => crate::i18n::Lang::En,
            _ => crate::i18n::detect(),
        });
        crate::ui::keymap::init(&config.keys)?;
        crate::ui::init_theme_overrides(&config.theme)?;
        crate::ui::init_editor(config.ui.editor.clone());

        let verbose = self.verbose || config.ui.verbose.unwrap_or(false);
        let dir = self.repo.unwrap_or_else(|| PathBuf::from("."));
        // Theme precedence: explicit CLI option > configuration > terminal detection.
        let theme = match self.theme {
            ThemeArg::Auto => config.ui.theme.unwrap_or(ThemeArg::Auto),
            explicit => explicit,
        };
        let light = match theme {
            ThemeArg::Light => true,
            ThemeArg::Dark => false,
            ThemeArg::Auto => crate::ui::detect_light(),
        };
        match self.command {
            None => commands::menu::run(verbose, &dir, light),
            Some(Commands::Merge(args)) => commands::run::merge(args, verbose, &dir, light),
            Some(Commands::Rebase(args)) => commands::run::rebase(args, verbose, &dir, light),
            Some(Commands::Pull(args)) => commands::run::pull(args, verbose, &dir, light),
            Some(Commands::CherryPick(args)) => {
                commands::run::cherry_pick(args, verbose, &dir, light)
            }
            Some(Commands::Revert(args)) => commands::run::revert(args, verbose, &dir, light),
            Some(Commands::Switch(args)) => commands::switch::run(args, verbose, &dir, light),
            Some(Commands::File(args)) => commands::file::run(args, light),
            Some(Commands::Abort) => commands::abort::run(verbose, &dir),
            // Already handled before configuration loading.
            Some(Commands::Completions(_)) => Ok(()),
        }
    }
}
