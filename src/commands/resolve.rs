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
///
/// 全部 stage 的 blob 用一个 `git cat-file --batch` 进程按序批量读回,
/// 避免逐文件逐 stage spawn 进程。
fn build_session(git: &Git, files: &[ConflictedFile]) -> Result<Session> {
    let state = git.state()?;
    let oids: Vec<&str> = files
        .iter()
        .flat_map(|f| [f.base.as_deref(), f.ours.as_deref(), f.theirs.as_deref()])
        .flatten()
        .collect();
    let mut blobs = git.read_blobs(&oids)?.into_iter();
    // 应答与请求同序:缺失的 stage 不在请求中,取空内容
    let mut take = |oid: &Option<String>| -> Vec<u8> {
        if oid.is_some() {
            blobs.next().unwrap_or_default()
        } else {
            Vec::new()
        }
    };

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
        let base = take(&file.base);
        let ours = take(&file.ours);
        let theirs = take(&file.theirs);
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
