//! Switch to another branch; remote branches get a local tracking branch first.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Result, bail};
use clap::Args;

use crate::git::{Git, GitError};
use crate::i18n::{tr, tr_f};
use crate::ui::{self, MenuItem};

/// Arguments for the switch subcommand.
#[derive(Debug, Args)]
pub struct SwitchArgs {
    /// Target branch (local or remote, such as origin/foo); opens a picker if omitted
    pub branch: Option<String>,
}

/// Switch to the given branch, or open a branch picker when no target is supplied.
pub fn run(args: SwitchArgs, verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    let target = match args.branch {
        Some(branch) => branch,
        None => match pick_branch(&git, light)? {
            Some(branch) => branch,
            // The user cancelled the picker.
            None => return Ok(()),
        },
    };
    let cmd = switch_cmd(&git, &target)?;
    let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
    println!("[git-pincer] $ git {}", cmd.join(" "));
    // Inherit terminal output so Git reports both success and failure.
    let status = git.run_inherit(&refs)?;
    if !status.success() {
        bail!("{}", tr_f("switch.failed", &[("branch", &target)]));
    }
    Ok(())
}

/// Build a switch command: switch directly to local branches, or use `--track`
/// to create a local branch for remote targets. Let Git resolve other names or report errors.
pub fn switch_cmd(git: &Git, target: &str) -> Result<Vec<String>, GitError> {
    if git.ref_exists(&format!("refs/heads/{target}"))? {
        return Ok(vec!["switch".to_owned(), target.to_owned()]);
    }
    if git.ref_exists(&format!("refs/remotes/{target}"))? {
        return Ok(vec![
            "switch".to_owned(),
            "--track".to_owned(),
            target.to_owned(),
        ]);
    }
    Ok(vec!["switch".to_owned(), target.to_owned()])
}

/// Open the main menu's branch picker; return None if the user cancels.
fn pick_branch(git: &Git, light: bool) -> Result<Option<String>> {
    if !std::io::stdout().is_terminal() {
        bail!("{}", tr("common.need_tty_menu"));
    }
    let items = branch_items(&git.list_switch_branches()?);
    if items.is_empty() {
        println!("[git-pincer] {}", tr("menu.no_branches"));
        return Ok(None);
    }
    // Dropping the session restores the terminal before subsequent Git output.
    let mut session = ui::MenuSession::open(light)?;
    let Some(idx) = session.pick(tr("menu.pick_switch"), &items, None, 0)? else {
        return Ok(None);
    };
    Ok(Some(items[idx].label.clone()))
}

/// Build picker entries with local branches first, followed by remote branches with prefixes.
pub(crate) fn branch_items(branches: &crate::git::SwitchBranches) -> Vec<MenuItem> {
    branches
        .locals
        .iter()
        .map(|b| MenuItem::new(b.clone(), ""))
        .chain(
            branches
                .remotes
                .iter()
                .map(|b| MenuItem::new(b.clone(), tr("menu.remote_mark"))),
        )
        .collect()
}
