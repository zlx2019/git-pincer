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

/// 界面语言选择(命令行与配置文件 `[ui] lang` 共用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LangArg {
    /// 按系统 locale 自动选择(zh 前缀用中文,其余英文)
    Auto,
    /// 中文
    Zh,
    /// 英文
    En,
}

/// 界面主题选择(命令行与配置文件 `[ui] theme` 共用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
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
    /// Generate a shell completion script to stdout
    Completions(commands::completions::CompletionsArgs),
}

impl Cli {
    /// Distribute and execute the selected subcommands.
    pub fn run(self) -> Result<()> {
        // 补全脚本生成先于配置加载:它随 shell 启动执行,
        // 不应被配置文件错误或 git 环境问题阻断
        if let Some(Commands::Completions(args)) = &self.command {
            commands::completions::run(args.shell);
            return Ok(());
        }
        // --lang 显式指定时最早生效,让配置文件自身的报错也用对语言;
        // i18n::init 首次调用生效,因此调用顺序即优先级:命令行 > 配置 > 系统探测
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
        // 主题:命令行显式指定 > 配置文件 > 终端环境探测
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
            Some(Commands::File(args)) => commands::file::run(args, light),
            Some(Commands::Abort) => commands::abort::run(verbose, &dir),
            // 已在配置加载前处理并早退
            Some(Commands::Completions(_)) => Ok(()),
        }
    }
}
