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

/// 界面语言选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LangArg {
    /// 按系统 locale 自动选择(zh 前缀用中文,其余英文)
    Auto,
    /// 中文
    Zh,
    /// 英文
    En,
}

/// 界面主题选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ThemeArg {
    /// 按终端环境自动选择(读 COLORFGBG,检测不到用深色)
    Auto,
    /// 深色(Tokyo Night)
    Dark,
    /// 浅色(Maple Light,适配浅色终端背景)
    Light,
}

/// 支持的子命令。
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
    /// Resolve a single conflict-marked file, no git required
    File(commands::file::FileArgs),
    /// Abort the operation in progress (merge / rebase / cherry-pick / revert / am)
    Abort,
}

impl Cli {
    /// Distribute and execute the selected subcommands.
    pub fn run(self) -> Result<()> {
        crate::i18n::init(match self.lang {
            LangArg::Zh => crate::i18n::Lang::Zh,
            LangArg::En => crate::i18n::Lang::En,
            LangArg::Auto => crate::i18n::detect(),
        });
        let verbose = self.verbose;
        let dir = self.repo.unwrap_or_else(|| PathBuf::from("."));
        let light = match self.theme {
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
            Some(Commands::File(args)) => commands::file::run(args, light),
            Some(Commands::Abort) => commands::abort::run(verbose, &dir),
        }
    }
}
