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
            "git {} 失败(并非冲突导致),请查看上方 git 输出",
            initial.join(" ")
        );
    }
    resolve_loop(&git, light)
}

/// 菜单模式编排:捕获输出执行 git 操作,非冲突失败返回 `Some(错误信息)`
/// 供弹框展示而非终止程序;成功或冲突已解决返回 `None`。
///
/// 成功与冲突路径把捕获的 git 输出回放到终端(历史可查);
/// 失败路径不回放详情——原因由弹框独家展示,历史只留一行结论,
/// 避免同一份错误信息出现两遍。
pub fn try_launch(
    initial: &[&str],
    verbose: bool,
    dir: &Path,
    light: bool,
) -> Result<Option<String>> {
    let git = Git::discover(dir, verbose)?;
    println!("[git-pincer] $ git {}", initial.join(" "));
    let out = git.run(initial)?;
    let replay = || {
        print!("{}", String::from_utf8_lossy(&out.stdout));
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
    };
    if out.status.success() {
        replay();
        return Ok(None);
    }
    if git.conflicted_files()?.is_empty() {
        println!("[git-pincer] ✗ git {} 失败", initial.join(" "));
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let reason = if stderr.is_empty() { stdout } else { stderr };
        return Ok(Some(reason));
    }
    replay();
    resolve_loop(&git, light)?;
    Ok(None)
}
