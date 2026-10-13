//! The shared conflict-resolution loop behind every entry point
//! (bare invocation, merge / rebase / pull / cherry-pick / revert).

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;

use crate::app::{FileEntry, FileMerge, PickKind, Session, Side};
use crate::git::{ConflictedFile, Git, RepoState, Stage};
use crate::i18n::{tr, tr_f};
use crate::ui::{self, Outcome};

/// Resolve all files, run `--continue`, and probe again until the operation finishes.
/// A rebase may require multiple rounds of conflict resolution.
pub fn resolve_loop(git: &Git, light: bool) -> Result<()> {
    loop {
        let files = git.conflicted_files()?;
        if files.is_empty() {
            let state = git.state()?;
            if state == RepoState::Clean {
                println!("[git-pincer] {}", tr("resolve.all_done"));
                return Ok(());
            }
            // Stream Git and hook output directly to the terminal.
            println!("[git-pincer] $ git {} --continue", state.op_name());
            let status = git.continue_op(state)?;
            if status.success() {
                continue;
            }
            // A nonzero exit may mean the next rebase commit conflicted; otherwise report failure.
            if git.conflicted_files()?.is_empty() {
                bail!(
                    "{}",
                    tr_f("resolve.continue_failed", &[("op", state.op_name())])
                );
            }
            println!("[git-pincer] {}", tr("resolve.next_round"));
            continue;
        }

        println!(
            "[git-pincer] {}",
            tr_f("resolve.found", &[("n", &files.len().to_string())])
        );
        let mut session = build_session(git, &files, false)?;
        let outcome = ui::run_session(
            &mut session,
            &mut |entry: &FileEntry| persist(git, &files, entry),
            light,
        )?;
        if outcome == Outcome::Quit {
            println!("[git-pincer] {}", tr("resolve.quit"));
            return Ok(());
        }
    }
}

/// Resolve conflicts from the menu using its transferred terminal for the first round.
/// Stay in the alternate screen to avoid flicker during the transition.
///
/// Replay the command and captured Git output after the first session restores
/// the terminal, before any streamed `--continue` output, preserving output order.
/// Subsequent rounds use the regular [`resolve_loop`].
pub(crate) fn resolve_from_menu(
    git: &Git,
    light: bool,
    terminal: DefaultTerminal,
    cmd_line: &str,
    captured: &std::process::Output,
) -> Result<()> {
    let files = git.conflicted_files()?;
    let outcome = if files.is_empty() {
        // If conflicts disappeared (for example, due to a race), restore and use the regular loop.
        ratatui::restore();
        None
    } else {
        // Restore the terminal on session construction failure so errors remain visible.
        match build_session(git, &files, true) {
            Ok(mut session) => Some(ui::run_session_in(
                terminal,
                &mut session,
                &mut |entry: &FileEntry| persist(git, &files, entry),
                light,
            )?),
            Err(e) => {
                ratatui::restore();
                return Err(e);
            }
        }
    };

    // The terminal is restored; replay the command and captured Git output.
    println!("[git-pincer] $ git {cmd_line}");
    print!("{}", String::from_utf8_lossy(&captured.stdout));
    eprint!("{}", String::from_utf8_lossy(&captured.stderr));
    if outcome == Some(Outcome::Quit) {
        println!("[git-pincer] {}", tr("resolve.quit"));
        return Ok(());
    }
    resolve_loop(git, light)
}

/// Persist a resolved file: write and stage text, or let Git check out the chosen
/// side of a whole-file pick (removing the file if that side is deleted).
pub fn persist(git: &Git, files: &[ConflictedFile], entry: &FileEntry) -> Result<()> {
    match entry {
        FileEntry::Text(merge) => {
            git.stage_resolved(&merge.path, merge.resolved_content().as_bytes())?;
        }
        FileEntry::Pick { path, choice, .. } => {
            let file = files
                .iter()
                .find(|f| f.path == *path)
                .with_context(|| format!("{path} is not in the conflict list"))?;
            let stage = match choice {
                Some(Side::Ours) => 2,
                Some(Side::Theirs) => 3,
                None => bail!("{path} has no side picked"),
            };
            git.take_side(file, stage)?;
        }
    }
    Ok(())
}

/// Build a session from conflicted files and their three index stages. Submodules,
/// symlinks, modify/delete conflicts, and NUL-containing binaries use whole-file picks.
///
/// Read all stage blobs in order through one `git cat-file --batch` process.
/// `quiet` suppresses progress output while the menu's alternate screen is active,
/// preventing printed messages from corrupting the display.
pub fn build_session(git: &Git, files: &[ConflictedFile], quiet: bool) -> Result<Session> {
    let state = git.state()?;
    let oids: Vec<&str> = files
        .iter()
        // Submodule commits may not exist in this object database and would fail the session.
        .filter(|f| !f.is_submodule())
        .flat_map(|f| [&f.base, &f.ours, &f.theirs])
        .flatten()
        .map(|stage| stage.oid.as_str())
        .collect();
    let mut blobs = git.read_blobs(&oids)?.into_iter();
    // Responses follow request order; missing stages were not requested and map to None.
    let mut take = |stage: &Option<Stage>| -> Option<Vec<u8>> {
        stage.as_ref().map(|_| blobs.next().unwrap_or_default())
    };

    let mut entries = Vec::new();
    for (index, file) in files.iter().enumerate() {
        if !quiet {
            println!(
                "{}",
                tr_f(
                    "resolve.reading",
                    &[
                        ("i", &(index + 1).to_string()),
                        ("n", &files.len().to_string()),
                        ("path", &file.path),
                    ],
                )
            );
        }
        let path = file.path.clone();
        // Submodule stages were not requested; display each side's recorded commit ID.
        if file.is_submodule() {
            let commit = |stage: &Option<Stage>| stage.as_ref().map(|s| s.oid.clone().into_bytes());
            entries.push(FileEntry::Pick {
                path,
                kind: PickKind::Submodule,
                ours: commit(&file.ours),
                theirs: commit(&file.theirs),
                choice: None,
            });
            continue;
        }
        let base = take(&file.base);
        let ours = take(&file.ours);
        let theirs = take(&file.theirs);
        // Symlink targets cannot be merged as text or written through the filesystem.
        if file.is_symlink() {
            entries.push(FileEntry::Pick {
                path,
                kind: PickKind::Symlink,
                ours,
                theirs,
                choice: None,
            });
            continue;
        }
        // Modify/delete conflicts require a whole-file choice, as in IDEA / git mergetool.
        // The deleted side is represented by None.
        if ours.is_none() || theirs.is_none() {
            entries.push(FileEntry::Pick {
                path,
                kind: PickKind::File,
                ours,
                theirs,
                choice: None,
            });
            continue;
        }
        let (base, ours, theirs) = (
            base.unwrap_or_default(),
            ours.unwrap_or_default(),
            theirs.unwrap_or_default(),
        );
        if base.contains(&0) || ours.contains(&0) || theirs.contains(&0) {
            entries.push(FileEntry::Pick {
                path,
                kind: PickKind::File,
                ours: Some(ours),
                theirs: Some(theirs),
                choice: None,
            });
        } else {
            entries.push(FileEntry::Text(FileMerge::from_three_way(
                path,
                &String::from_utf8_lossy(&base),
                &String::from_utf8_lossy(&ours),
                &String::from_utf8_lossy(&theirs),
            )));
        }
    }
    Ok(Session::new(entries, state.op_name().to_owned()))
}
