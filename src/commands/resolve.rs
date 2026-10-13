//! The shared conflict-resolution loop behind every entry point
//! (bare invocation, merge / rebase / pull / cherry-pick / revert).

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;

use crate::app::{FileEntry, FileMerge, PickKind, Session, Side};
use crate::git::{ConflictedFile, Git, RepoState, Stage};
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

/// 菜单转入的冲突解决:复用移交的终端直接进入第一轮会话,
/// 全程不退出 alternate screen(消除菜单 → 冲突界面的闪屏)。
///
/// 命令行与捕获的 git 输出在首个会话结束、终端恢复之后才补进
/// 滚动历史,先于任何 `--continue` 的透传输出,顺序与原流程一致;
/// 后续轮次(多轮 rebase 等)交回常规 [`resolve_loop`]。
pub(crate) fn resolve_from_menu(
    git: &Git,
    light: bool,
    terminal: DefaultTerminal,
    cmd_line: &str,
    captured: &std::process::Output,
) -> Result<()> {
    let files = git.conflicted_files()?;
    let outcome = if files.is_empty() {
        // 兜底:已无冲突可解(如探测竞态),恢复终端走常规流程
        ratatui::restore();
        None
    } else {
        // 会话构建失败也必须恢复终端,否则错误信息会丢在 alternate screen 里
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

    // 终端已恢复:补回命令历史与捕获的 git 输出
    println!("[git-pincer] $ git {cmd_line}");
    print!("{}", String::from_utf8_lossy(&captured.stdout));
    eprint!("{}", String::from_utf8_lossy(&captured.stderr));
    if outcome == Some(Outcome::Quit) {
        println!("[git-pincer] {}", tr("resolve.quit"));
        return Ok(());
    }
    resolve_loop(git, light)
}

/// 落盘一个已解决的文件:符号链接 / 子模块经 git 整条取用所选一侧
/// (不经文件系统写入),其余写入解决结果并 `git add`。
pub fn persist(git: &Git, files: &[ConflictedFile], entry: &FileEntry) -> Result<()> {
    if let FileEntry::Pick {
        path,
        kind: PickKind::Symlink | PickKind::Submodule,
        choice: Some(side),
        ..
    } = entry
    {
        let file = files
            .iter()
            .find(|f| f.path == *path)
            .with_context(|| format!("{path} is not in the conflict list"))?;
        let stage = match side {
            Side::Ours => 2,
            Side::Theirs => 3,
        };
        git.take_side(file, stage)?;
    } else {
        git.stage_resolved(entry.path(), &entry.resolved_bytes())?;
    }
    Ok(())
}

/// 由冲突文件列表构建会话:读取三份 stage 内容;子模块、符号链接与含 NUL 的
/// 二进制文件不能逐块合并,降级为整条二选一。
///
/// 全部 stage 的 blob 用一个 `git cat-file --batch` 进程按序批量读回,
/// 避免逐文件逐 stage spawn 进程。`quiet` 抑制逐文件进度打印
/// (菜单移交终端时 alternate screen 仍活跃,打印会破坏画面)。
pub fn build_session(git: &Git, files: &[ConflictedFile], quiet: bool) -> Result<Session> {
    let state = git.state()?;
    let oids: Vec<&str> = files
        .iter()
        // 子模块的提交不在本仓库对象库中,读取会失败并阻塞整轮会话
        .filter(|f| !f.is_submodule())
        .flat_map(|f| [&f.base, &f.ours, &f.theirs])
        .flatten()
        .map(|stage| stage.oid.as_str())
        .collect();
    let mut blobs = git.read_blobs(&oids)?.into_iter();
    // 应答与请求同序:缺失的 stage 不在请求中,取 None
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
        // 子模块:不读取内容(其 stage 不在批量请求中),两侧以记录的提交 id 展示
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
        // 符号链接:链接目标不能按文本合并,写盘也不能经文件系统(会写穿链接)
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
