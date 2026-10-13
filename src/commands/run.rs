//! merge / rebase / pull: launch a git operation and take over the conflict resolution that follows.

use std::path::Path;

use anyhow::{Result, bail};
use clap::Args;

use super::resolve::resolve_loop;
use crate::git::Git;

/// Arguments for the merge subcommand.
#[derive(Debug, Args)]
pub struct MergeArgs {
    /// Target reference to merge into the current branch (branch / tag / commit)
    pub target: String,
}

/// Arguments for the rebase subcommand.
#[derive(Debug, Args)]
pub struct RebaseArgs {
    /// Target reference to rebase onto
    pub target: String,
}

/// Arguments for the pull subcommand, passed through to git pull.
#[derive(Debug, Args)]
pub struct PullArgs {
    /// Arguments passed to git pull, such as `origin main` / `--rebase`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Arguments for cherry-pick, passed through with support for multiple commits and options.
#[derive(Debug, Args)]
pub struct CherryPickArgs {
    /// Arguments passed to git cherry-pick, such as `abc123` / `-x A B`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub args: Vec<String>,
}

/// Arguments for revert, passed through with support for multiple commits and options.
#[derive(Debug, Args)]
pub struct RevertArgs {
    /// Arguments passed to git revert, such as `abc123` / `HEAD~2..HEAD`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub args: Vec<String>,
}

/// Run `git merge` and take over conflict resolution.
pub fn merge(args: MergeArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    launch(&["merge", "--no-edit", &args.target], verbose, dir, light)
}

/// Run `git rebase` and take over conflict resolution.
pub fn rebase(args: RebaseArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    launch(&["rebase", &args.target], verbose, dir, light)
}

/// Run `git pull` with arguments passed through and take over conflict resolution.
pub fn pull(args: PullArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let mut cmd = vec!["pull"];
    cmd.extend(args.args.iter().map(String::as_str));
    launch(&cmd, verbose, dir, light)
}

/// Run `git cherry-pick` with arguments passed through, resolving each conflicted round.
pub fn cherry_pick(args: CherryPickArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let mut cmd = vec!["cherry-pick"];
    cmd.extend(args.args.iter().map(String::as_str));
    launch(&cmd, verbose, dir, light)
}

/// Run `git revert` with arguments passed through and take over conflict resolution.
pub fn revert(args: RevertArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let mut cmd = vec!["revert", "--no-edit"];
    cmd.extend(args.args.iter().map(String::as_str));
    launch(&cmd, verbose, dir, light)
}

/// Run Git with inherited output, then enter the resolution loop if conflicts occur.
fn launch(initial: &[&str], verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    println!("[git-pincer] $ git {}", initial.join(" "));
    let status = git.run_inherit(initial)?;
    if status.success() {
        // Git has already printed the operation's result.
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

/// Result of one Git operation in menu mode.
#[derive(Debug)]
pub enum LaunchOutcome {
    /// Completed without conflicts, with captured output for replay
    Success(std::process::Output),
    /// Conflicts awaiting resolution, with captured output for replay
    Conflicts(std::process::Output),
    /// Failure unrelated to conflicts, with a reason to display in a dialog
    Failed(String),
}

/// Run a Git operation with captured output and classify the result for menu mode.
///
/// Does not print or open a TUI, so it can run while the menu session is active.
/// Failure dialogs can appear without restarting the TUI and causing flicker.
/// The caller replays the output after leaving the TUI.
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
