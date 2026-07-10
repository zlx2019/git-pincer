//! completions subcommand: generate shell completion scripts to stdout.

use clap::CommandFactory;
use clap_complete::Shell;

/// completions 子命令参数。
#[derive(Debug, clap::Args)]
pub struct CompletionsArgs {
    /// 目标 shell(bash / zsh / fish / powershell / elvish)
    #[arg(value_enum)]
    pub shell: Shell,
}

/// 生成补全脚本并写入标准输出,供 shell 初始化脚本 source。
pub fn run(shell: Shell) {
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "git-pincer", &mut std::io::stdout());
}
