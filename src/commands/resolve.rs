//! Take over the conflicts already present in a repository
//! (also the shared orchestration core behind merge / rebase / pull).

use anyhow::{Result, bail};
use std::path::Path;

use crate::app::{FileEntry, FileMerge, Session};
use crate::git::{ConflictedFile, Git, RepoState};
use crate::ui::{self, Outcome};

/// `git-peace`(无子命令):接管指定仓库已有的冲突现场。
pub fn run(verbose: bool, dir: &Path) -> Result<()> {
    let git = Git::discover(dir, verbose)?;
    if git.conflicted_files()?.is_empty() && git.state()? == RepoState::Clean {
        println!("Currently, there are no conflicts that need to be resolved.");
        return Ok(());
    }
    resolve_loop(&git)
}

/// 冲突解决主循环:解决全部文件 → `--continue` → 重新探测,
/// 直到仓库回到干净状态(rebase 可能经历多轮冲突)。
pub fn resolve_loop(git: &Git) -> Result<()> {
    loop {
        let files = git.conflicted_files()?;
        if files.is_empty() {
            let state = git.state()?;
            if state == RepoState::Clean {
                println!("[git-peace] ✔ 全部完成,仓库已回到干净状态");
                return Ok(());
            }
            println!(
                "[git-peace] 冲突已全部解决,执行 git {} --continue …",
                state.op_name()
            );
            let out = git.continue_op(state)?;
            if out.status.success() {
                continue;
            }
            // 非零退出:rebase 的下一个 commit 又冲突了则继续循环,否则如实报错。
            // 钩子输出多在 stdout(如 pre-commit 的 fmt/clippy),需要一并展示
            if git.conflicted_files()?.is_empty() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                bail!(
                    "git {} --continue 失败:\n{}\n{}\n\n提示:若是 pre-commit 钩子拒绝提交(fmt / clippy 等),\
                     修复问题并 git add 后重新运行 git-peace 即可继续,已解决的冲突不会丢失",
                    state.op_name(),
                    stdout.trim(),
                    stderr.trim()
                );
            }
            println!("[git-peace] 进入下一轮,仍有冲突待解决");
            continue;
        }

        println!("[git-peace] {} 个文件存在冲突,进入解决界面…", files.len());
        let mut session = build_session(git, &files)?;
        let outcome = ui::run_session(&mut session, &mut |path: &str, bytes: &[u8]| {
            git.stage_resolved(path, bytes)?;
            Ok(())
        })?;
        if outcome == Outcome::Quit {
            println!("[git-peace] 已退出,现场保留;随时运行 git-peace 继续,或 git-peace abort 中止");
            return Ok(());
        }
    }
}

/// 由冲突文件列表构建会话:读取三份 stage 内容,含 NUL 的按二进制降级处理。
fn build_session(git: &Git, files: &[ConflictedFile]) -> Result<Session> {
    let state = git.state()?;
    let mut entries = Vec::new();
    for (index, file) in files.iter().enumerate() {
        println!("[{}/{}] 读取 {} …", index + 1, files.len(), file.path);
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
