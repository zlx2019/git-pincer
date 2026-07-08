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
    /// 子命令, 缺省时处理已有的冲突
    #[command(subcommand)]
    pub command: Option<Commands>,
    /// 要处理的 Git 仓库路径(默认为当前工作目录)
    #[arg(short = 'C', long = "repo", global = true, value_name = "PATH")]
    pub repo: Option<PathBuf>,
    /// 回显执行的 git 命令(可重复以提高级别,如 `-vv`)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// 使用浅色主题(适配浅色终端背景)
    #[arg(long, global = true, conflicts_with = "dark")]
    pub light: bool,
    /// 使用深色主题(覆盖 COLORFGBG 自动检测;默认即深色)
    #[arg(long, global = true)]
    pub dark: bool,
}

/// 支持的子命令。
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// 执行 git merge(存在冲突则进行处理)
    Merge(commands::run::MergeArgs),
    /// 执行 git rebase
    Rebase(commands::run::RebaseArgs),
    /// 执行 git pull
    Pull(commands::run::PullArgs),
    /// 处理单个带有冲突标记的文件
    File(commands::file::FileArgs),
    /// 中止进行中的 merge / rebase
    Abort,
}

impl Cli {
    /// Distribute and execute the selected subcommands.
    pub fn run(self) -> Result<()> {
        let verbose = self.verbose > 0;
        let dir = self.repo.unwrap_or_else(|| PathBuf::from("."));
        // 主题变体:显式参数优先,否则按 COLORFGBG 推断(缺省深色)
        let light = if self.light {
            true
        } else if self.dark {
            false
        } else {
            crate::ui::detect_light()
        };
        match self.command {
            None => commands::resolve::run(verbose, &dir, light),
            Some(Commands::Merge(args)) => commands::run::merge(args, verbose, &dir, light),
            Some(Commands::Rebase(args)) => commands::run::rebase(args, verbose, &dir, light),
            Some(Commands::Pull(args)) => commands::run::pull(args, verbose, &dir, light),
            Some(Commands::File(args)) => commands::file::run(args, light),
            Some(Commands::Abort) => commands::abort::run(verbose, &dir),
        }
    }
}
