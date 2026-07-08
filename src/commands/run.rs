//! merge / rebase / pull: launch a git operation and take over the conflict resolution that follows.

use std::path::Path;

use anyhow::{Result, bail};
use clap::Args;

use super::resolve::resolve_loop;
use crate::git::Git;

/// merge 子命令参数。
#[derive(Debug, Args)]
pub struct MergeArgs {
    /// 要合并进当前分支的目标引用(分支 / tag / commit)
    pub target: String,
}

/// rebase 子命令参数。
#[derive(Debug, Args)]
pub struct RebaseArgs {
    /// 变基的目标引用
    pub target: String,
}

/// pull 子命令参数(全部透传给 git pull)。
#[derive(Debug, Args)]
pub struct PullArgs {
    /// 透传给 git pull 的参数,如 `origin main` / `--rebase`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// cherry-pick 子命令参数(全部透传,可带多个提交与选项)。
#[derive(Debug, Args)]
pub struct CherryPickArgs {
    /// 透传给 git cherry-pick 的参数,如 `abc123` / `-x A B`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub args: Vec<String>,
}

/// revert 子命令参数(全部透传,可带多个提交与选项)。
#[derive(Debug, Args)]
pub struct RevertArgs {
    /// 透传给 git revert 的参数,如 `abc123` / `HEAD~2..HEAD`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub args: Vec<String>,
}

/// 执行 `git merge` 并接管冲突解决。
pub fn merge(args: MergeArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    launch(&["merge", "--no-edit", &args.target], verbose, dir, light)
}

/// 执行 `git rebase` 并接管冲突解决。
pub fn rebase(args: RebaseArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    launch(&["rebase", &args.target], verbose, dir, light)
}

/// 执行 `git pull`(参数透传)并接管冲突解决。
pub fn pull(args: PullArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let mut cmd = vec!["pull"];
    cmd.extend(args.args.iter().map(String::as_str));
    launch(&cmd, verbose, dir, light)
}

/// 执行 `git cherry-pick`(参数透传)并接管冲突解决(多提交会多轮循环)。
pub fn cherry_pick(args: CherryPickArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let mut cmd = vec!["cherry-pick"];
    cmd.extend(args.args.iter().map(String::as_str));
    launch(&cmd, verbose, dir, light)
}

/// 执行 `git revert`(参数透传)并接管冲突解决。
pub fn revert(args: RevertArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let mut cmd = vec!["revert", "--no-edit"];
    cmd.extend(args.args.iter().map(String::as_str));
    launch(&cmd, verbose, dir, light)
}

/// 共享编排:透传执行 git 操作;干净则结束,产生冲突则进入解决循环。
fn launch(initial: &[&str], verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    println!("[git-pincer] $ git {}", initial.join(" "));
    let status = git.run_inherit(initial)?;
    if status.success() {
        // git 已输出自身的合并结果,不再重复旁白
        return Ok(());
    }
    if git.conflicted_files()?.is_empty() {
        bail!(
            "git {} execution failed, please check the Git output messages",
            initial.join(" ")
        );
    }
    resolve_loop(&git, light)
}

/// 菜单模式的一次 git 执行结果。
#[derive(Debug)]
pub enum LaunchOutcome {
    /// 顺利完成,无冲突(附捕获的输出,供回放)
    Success(std::process::Output),
    /// 产生冲突,等待接管解决(附捕获的输出,供回放)
    Conflicts(std::process::Output),
    /// 非冲突失败(附整理后的失败原因,供弹框展示)
    Failed(String),
}

/// 菜单模式编排:捕获输出执行 git 操作并判定结果。
///
/// 不打印、不进入任何 TUI,因此可以在菜单会话仍打开时调用
/// (失败弹框无需退出再重进 TUI,避免闪屏);
/// 输出的回放时机由调用方决定(离开 TUI 之后)。
pub fn launch_captured(git: &Git, initial: &[&str]) -> Result<LaunchOutcome> {
    let out = git.run(initial)?;
    if out.status.success() {
        return Ok(LaunchOutcome::Success(out));
    }
    if git.conflicted_files()?.is_empty() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let reason = if stderr.is_empty() { stdout } else { stderr };
        return Ok(LaunchOutcome::Failed(reason));
    }
    Ok(LaunchOutcome::Conflicts(out))
}
