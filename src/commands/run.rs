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

/// 执行 `git merge` 并接管冲突解决。
pub fn merge(args: MergeArgs, verbose: bool, dir: &Path) -> Result<()> {
    launch(&["merge", "--no-edit", &args.target], verbose, dir)
}

/// 执行 `git rebase` 并接管冲突解决。
pub fn rebase(args: RebaseArgs, verbose: bool, dir: &Path) -> Result<()> {
    launch(&["rebase", &args.target], verbose, dir)
}

/// 执行 `git pull`(参数透传)并接管冲突解决。
pub fn pull(args: PullArgs, verbose: bool, dir: &Path) -> Result<()> {
    let mut cmd = vec!["pull"];
    cmd.extend(args.args.iter().map(String::as_str));
    launch(&cmd, verbose, dir)
}

/// 共享编排:透传执行 git 操作;干净则结束,产生冲突则进入解决循环。
fn launch(initial: &[&str], verbose: bool, dir: &Path) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    println!("[git-peace] $ git {}", initial.join(" "));
    let status = git.run_inherit(initial)?;
    if status.success() {
        println!("[git-peace] ✔ 完成,无冲突");
        return Ok(());
    }
    if git.conflicted_files()?.is_empty() {
        bail!(
            "git {} 失败(并非冲突导致),请查看上方 git 输出",
            initial.join(" ")
        );
    }
    resolve_loop(&git)
}
