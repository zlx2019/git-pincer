//! Bare `git-pincer` entry: take over active conflicts or open the action menu.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::Result;

use crate::git::{Git, RepoState};
use crate::i18n::{tr, tr_f};
use crate::ui::{self, MenuItem};

use super::resolve::{resolve_from_menu, resolve_loop};
use super::run;

/// Maximum number of commits displayed in the commit picker.
const COMMIT_LIMIT: usize = 50;

/// Delay before showing the running page for an unfinished command.
/// Fast local commands such as merge or revert show their results immediately;
/// slower commands such as network pulls still provide execution feedback.
const FLASH_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// Run the bare command: take over an operation or enter the menu.
pub fn run(verbose: bool, dir: &Path, light: bool) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    // Take over existing conflicts when the user invokes git-pincer after a failed operation.
    if !git.conflicted_files()?.is_empty() || git.state()? != RepoState::Clean {
        return resolve_loop(&git, light);
    }
    // Skip the interactive menu in scripts and pipelines.
    if !std::io::stdout().is_terminal() {
        println!("{}", tr("menu.no_conflicts"));
        return Ok(());
    }
    menu_loop(&git, light)
}

/// Menu loop: choose an action, then a target; cancelling the target returns to actions.
///
/// Selection, execution, and result dialogs share one [`ui::MenuSession`].
/// Git output is captured to avoid terminal interaction and screen flicker.
/// On conflicts, transfer terminal ownership to the resolver without leaving the
/// alternate screen; replay captured output after the first resolution session.
fn menu_loop(git: &Git, light: bool) -> Result<()> {
    let actions: Vec<MenuItem> = [
        ("pull", tr("menu.pull_desc"), tr("menu.pull_hint")),
        ("merge", tr("menu.merge_desc"), tr("menu.merge_hint")),
        ("rebase", tr("menu.rebase_desc"), tr("menu.rebase_hint")),
        (
            "cherry-pick",
            tr("menu.cherry_desc"),
            tr("menu.cherry_hint"),
        ),
        ("revert", tr("menu.revert_desc"), tr("menu.revert_hint")),
        ("switch", tr("menu.switch_desc"), tr("menu.switch_hint")),
    ]
    .into_iter()
    .map(|(label, desc, hint)| MenuItem::new(label, desc).with_hint(hint))
    .collect();

    // Preserve the selected action when returning from a target picker.
    let mut last_action = 0usize;
    let (cmd, out, terminal) = {
        let mut session = ui::MenuSession::open(light)?;
        loop {
            // Refresh repository stats each round so the status panel stays current.
            let vitals = git.vitals()?;
            let Some(action) = session.pick("", &actions, Some(&vitals), last_action)? else {
                return Ok(());
            };
            last_action = action;
            // Build the Git command; cancelling a target returns to the action menu.
            let cmd: Vec<String> = match action {
                0 => vec!["pull".to_owned()],
                1 | 2 => {
                    let branches: Vec<MenuItem> = git
                        .list_branches()?
                        .into_iter()
                        .map(|b| MenuItem::new(b, ""))
                        .collect();
                    if branches.is_empty() {
                        session.notice(tr("menu.notice_info"), tr("menu.no_branches"))?;
                        continue;
                    }
                    let title = if action == 1 {
                        tr("menu.pick_merge")
                    } else {
                        tr("menu.pick_rebase")
                    };
                    let Some(idx) = session.pick(title, &branches, None, 0)? else {
                        continue;
                    };
                    let target = branches[idx].label.clone();
                    if action == 1 {
                        vec!["merge".to_owned(), "--no-edit".to_owned(), target]
                    } else {
                        vec!["rebase".to_owned(), target]
                    }
                }
                5 => {
                    let items = super::switch::branch_items(&git.list_switch_branches()?);
                    if items.is_empty() {
                        session.notice(tr("menu.notice_info"), tr("menu.no_branches"))?;
                        continue;
                    }
                    let Some(idx) = session.pick(tr("menu.pick_switch"), &items, None, 0)? else {
                        continue;
                    };
                    // Switch directly to local branches; use --track for remote branches.
                    super::switch::switch_cmd(git, &items[idx].label)?
                }
                _ => {
                    let others_only = action == 3;
                    // Split --oneline entries into short hash and subject columns.
                    let commits: Vec<MenuItem> = git
                        .recent_commits(others_only, COMMIT_LIMIT)?
                        .iter()
                        .filter_map(|line| line.split_once(' '))
                        .map(|(hash, subject)| MenuItem::new(hash, subject))
                        .collect();
                    if commits.is_empty() {
                        session.notice(tr("menu.notice_info"), tr("menu.no_commits"))?;
                        continue;
                    }
                    let title = if others_only {
                        tr("menu.pick_cherry")
                    } else {
                        tr("menu.pick_revert")
                    };
                    let Some(idx) = session.pick(title, &commits, None, 0)? else {
                        continue;
                    };
                    let hash = commits[idx].label.clone();
                    if others_only {
                        vec!["cherry-pick".to_owned(), hash]
                    } else {
                        vec!["revert".to_owned(), "--no-edit".to_owned(), hash]
                    }
                }
            };
            // Capture output within the session and show a result dialog before returning.
            // Run on a worker thread; show the running page only after FLASH_DELAY.
            let outcome = {
                let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::scope(|scope| -> Result<run::LaunchOutcome> {
                    scope.spawn(move || {
                        let _ = tx.send(run::launch_captured(git, &refs));
                    });
                    match rx.recv_timeout(FLASH_DELAY) {
                        Ok(result) => result,
                        Err(_) => {
                            session.flash(&format!("$ git {}", cmd.join(" ")))?;
                            rx.recv()
                                .map_err(|_| anyhow::anyhow!("git worker disconnected"))?
                        }
                    }
                })?
            };
            match outcome {
                run::LaunchOutcome::Failed(reason) => {
                    session.notice(&tr_f("menu.failed", &[("cmd", &cmd[0])]), &reason)?;
                }
                run::LaunchOutcome::Success(out) => {
                    let body = compose_notice(
                        &String::from_utf8_lossy(&out.stdout),
                        &String::from_utf8_lossy(&out.stderr),
                    );
                    session.notice(&tr_f("menu.done", &[("cmd", &cmd[0])]), &body)?;
                }
                // Transfer terminal ownership directly to the conflict resolver.
                run::LaunchOutcome::Conflicts(out) => break (cmd, out, session.into_terminal()?),
            }
        }
    };

    // Enter the resolver on the transferred terminal without leaving the alternate screen.
    // resolve_from_menu replays the command and captured output after restoring the terminal.
    resolve_from_menu(git, light, terminal, &cmd.join(" "), &out)
}

/// Maximum lines in a success dialog; keep the tail when output exceeds the limit.
const NOTICE_LINES: usize = 15;

/// Build dialog text by joining stdout and stderr, truncating from the start if needed.
fn compose_notice(stdout: &str, stderr: &str) -> String {
    let mut text = stdout.trim_end().to_owned();
    let err = stderr.trim_end();
    if !err.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err);
    }
    if text.trim().is_empty() {
        return tr("menu.no_output").to_owned();
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > NOTICE_LINES {
        let skipped = lines.len() - NOTICE_LINES;
        format!(
            "{}\n{}",
            tr_f("menu.skipped", &[("n", &skipped.to_string())]),
            lines[skipped..].join("\n")
        )
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Show a placeholder for empty output.
    #[test]
    fn compose_notice_empty_output() {
        assert_eq!(compose_notice("", "  \n"), "(no output)");
    }

    /// Join stdout and stderr in order, trimming trailing whitespace.
    #[test]
    fn compose_notice_merges_streams() {
        assert_eq!(
            compose_notice("Already up to date.\n", "warning: x\n"),
            "Already up to date.\nwarning: x"
        );
    }

    /// Keep the tail of long output and report the omitted line count.
    #[test]
    fn compose_notice_truncates_long_output() {
        let long = (1..=20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let got = compose_notice(&long, "");
        assert!(got.starts_with("… (first 5 lines omitted)"));
        assert!(got.ends_with("line20"));
        assert_eq!(got.lines().count(), NOTICE_LINES + 1);
    }
}
