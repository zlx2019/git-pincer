//! The shared conflict-resolution loop behind every entry point
//! (bare invocation, merge / rebase / pull / cherry-pick / revert).

use anyhow::{Result, bail};

use crate::app::{FileEntry, FileMerge, Session};
use crate::git::{ConflictedFile, Git, RepoState};
use crate::i18n::{tr, tr_f};
use crate::ui::{self, Outcome};

/// 冲突解决主循环:解决全部文件 → `--continue` → 重新探测,
/// 直到仓库回到干净状态(rebase 可能经历多轮冲突)。
pub fn resolve_loop(git: &Git, light: bool) -> Result<()> {
    loop {
        let files = git.conflicted_files()?;
        if files.is_empty() {
            let state = git.state()?;
            if state == RepoState::Clean {
                println!("[git-pincer] {}", tr("resolve.all_done"));
                return Ok(());
            }
            // 透传执行,git 与钩子的输出实时流向终端
            println!("[git-pincer] $ git {} --continue", state.op_name());
            let status = git.continue_op(state)?;
            if status.success() {
                continue;
            }
            // 非零退出:rebase 的下一个 commit 又冲突了则继续循环,否则如实报错
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
        let mut session = build_session(git, &files)?;
        let outcome = ui::run_session(
            &mut session,
            &mut |path: &str, bytes: &[u8]| {
                git.stage_resolved(path, bytes)?;
                Ok(())
            },
            light,
        )?;
        if outcome == Outcome::Quit {
            println!("[git-pincer] {}", tr("resolve.quit"));
            return Ok(());
        }
    }
}

/// 由冲突文件列表构建会话:读取三份 stage 内容,含 NUL 的按二进制降级处理。
fn build_session(git: &Git, files: &[ConflictedFile]) -> Result<Session> {
    let state = git.state()?;
    let mut entries = Vec::new();
    for (index, file) in files.iter().enumerate() {
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
        let read = |present: bool, stage: u8| -> Result<Vec<u8>> {
            if present {
                Ok(git.read_stage(&file.path, stage)?)
            } else {
                Ok(Vec::new())
            }
        };
        let base = read(file.has_base, 1)?;
        let ours = read(file.has_ours, 2)?;
        let theirs = read(file.has_theirs, 3)?;
        if base.contains(&0) || ours.contains(&0) || theirs.contains(&0) {
            entries.push(FileEntry::Binary {
                path: file.path.clone(),
                ours,
                theirs,
                choice: None,
            });
        } else {
            entries.push(FileEntry::Text(FileMerge::from_three_way(
                file.path.clone(),
                &String::from_utf8_lossy(&base),
                &String::from_utf8_lossy(&ours),
                &String::from_utf8_lossy(&theirs),
            )));
        }
    }
    Ok(Session::new(entries, state.op_name().to_owned()))
}
