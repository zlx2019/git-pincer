//! ratatui three-pane rendering and the key-event loop.
//!
//! Layout: status bar / three bordered panes (local | result | remote)
//! / key hints / message line. Change chunks are tinted as bands on the panes
//! they touch, colored by change type like IDEA (blue = modified, green =
//! added, gray = deleted, red = conflict); the band disappears once a chunk
//! is resolved, the current chunk is highlighted, and `?` shows the full key
//! reference.
//!
//! 模块拆分:
//! - [`theme`][] — 颜色集中定义
//! - [`rows`][] — 渲染行数据结构与构建(折叠 / 占位)
//! - [`highlight`][] — 词级强调与语法高亮的计算与缓存
//! - [`panes`][] — 三栏正文渲染
//! - [`chrome`][] — 界面整体绘制(状态栏 / 提示条 / 帮助浮层 / 二进制视图)
//! - 本文件 — 事件主循环与按键分发

mod chrome;
mod highlight;
mod panes;
mod rows;
mod theme;

use std::io::IsTerminal;

use anyhow::{Context, Result};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::app::{FileEntry, Session, Side};

pub use chrome::draw;
pub(crate) use theme::detect_light;

/// 会话结束方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 所有文件已解决并写盘
    Completed,
    /// 用户中途退出(现场保留,可再次进入)
    Quit,
}

/// UI 的瞬时状态(消息条与浮层开关)。
#[derive(Debug, Default)]
pub struct UiState {
    /// 底部消息条内容
    pub message: String,
    /// 是否显示帮助浮层
    pub show_help: bool,
    /// 是否处于「再按一次 q 退出」的确认态
    pub pending_quit: bool,
    /// 界面主题
    pub(crate) theme: theme::Theme,
    /// 高亮信息缓存(词级强调 / 语法高亮)
    pub(crate) cache: highlight::HighlightCache,
    /// 状态修订号:改动合并内容的按键后自增,用于结果栏语法高亮的失效重算
    pub(crate) revision: u64,
}

/// 运行交互会话直至完成或退出。
///
/// `write_file` 负责把解决后的字节落盘(git 模式下还会顺带 `git add`);
/// `light` 为 true 时使用浅色主题(适配浅色终端背景)。
pub fn run_session(
    session: &mut Session,
    write_file: &mut dyn FnMut(&str, &[u8]) -> Result<()>,
    light: bool,
) -> Result<Outcome> {
    // ratatui::init 在无 TTY 时会直接 panic,这里先行拦截给出可读错误
    // (如在管道 / CI 中误运行时)
    if !std::io::stdout().is_terminal() {
        anyhow::bail!("打开冲突解决界面需要交互式终端(当前 stdout 不是 TTY)");
    }
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, session, write_file, light);
    ratatui::restore();
    result
}

/// 事件主循环:绘制 → 读键 → 更新状态。
fn event_loop(
    terminal: &mut DefaultTerminal,
    session: &mut Session,
    write_file: &mut dyn FnMut(&str, &[u8]) -> Result<()>,
    light: bool,
) -> Result<Outcome> {
    let mut ui = UiState {
        theme: theme::Theme::select(light),
        ..UiState::default()
    };
    loop {
        terminal.draw(|frame| draw(frame, session, &mut ui))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // 帮助浮层打开时,任意键关闭
        if ui.show_help {
            ui.show_help = false;
            continue;
        }
        // 除 q 外的任意键取消退出确认
        if key.code != KeyCode::Char('q') {
            ui.pending_quit = false;
        }
        ui.message.clear();

        match key.code {
            KeyCode::Char('q') => {
                if session.all_written() || ui.pending_quit {
                    return Ok(Outcome::Quit);
                }
                ui.pending_quit = true;
                ui.message = "仍有文件未完成;再按一次 q 退出(未写盘的进度将丢弃)".to_owned();
            }
            KeyCode::Char('?') => ui.show_help = true,
            KeyCode::Tab => session.next_file(),
            KeyCode::Char('z') => session.folded = !session.folded,
            KeyCode::Char('w') => {
                if write_current(session, write_file, &mut ui)? {
                    // 写盘会自动应用非冲突改动,结果栏内容可能变化
                    ui.revision += 1;
                    if session.all_written() {
                        return Ok(Outcome::Completed);
                    }
                }
            }
            KeyCode::Char('e') => {
                if let FileEntry::Text(merge) = session.current_file_mut() {
                    let initial = merge.current_content(merge.cursor);
                    if let Some(lines) = edit_lines(terminal, &initial)? {
                        merge.set_override(lines);
                        ui.revision += 1;
                        ui.message = "已用编辑结果覆写当前块".to_owned();
                    } else {
                        ui.message = "编辑已取消".to_owned();
                    }
                }
            }
            code => {
                if handle_file_key(session, code, &mut ui) {
                    ui.revision += 1;
                }
            }
        }
    }
}

/// 处理作用于当前文件的普通按键;返回是否改动了合并内容(结果栏高亮失效用)。
fn handle_file_key(session: &mut Session, code: KeyCode, ui: &mut UiState) -> bool {
    match session.current_file_mut() {
        FileEntry::Text(merge) => match code {
            KeyCode::Char('h') | KeyCode::Left => {
                merge.apply(Side::Ours);
                true
            }
            KeyCode::Char('l') | KeyCode::Right => {
                merge.apply(Side::Theirs);
                true
            }
            // x = 忽略当前块所有仍待处理的侧(已取用的内容保留)
            KeyCode::Char('x') => {
                merge.ignore(Side::Ours);
                merge.ignore(Side::Theirs);
                true
            }
            KeyCode::Char('u') => {
                merge.undo();
                true
            }
            KeyCode::Char('a') => {
                merge.apply_all_nonconflict();
                ui.message = "已应用全部非冲突改动".to_owned();
                true
            }
            KeyCode::Char('j') | KeyCode::Down => {
                merge.next_change();
                false
            }
            KeyCode::Char('k') | KeyCode::Up => {
                merge.prev_change();
                false
            }
            KeyCode::Char('n') => {
                merge.next_conflict();
                false
            }
            KeyCode::Char('p') => {
                merge.prev_conflict();
                false
            }
            // 复制键(终端框选会横跨三栏,复制键绕开这个限制):
            // y 块结果 / Y 整个文件结果 / H 块本地侧 / L 块远端侧
            KeyCode::Char('y') => {
                let lines = merge.current_content(merge.cursor);
                ui.message = copy_feedback(&lines, "当前块结果");
                false
            }
            KeyCode::Char('Y') => {
                ui.message = match copy_to_clipboard(&merge.resolved_content()) {
                    Ok(()) => "已复制整个文件的当前结果".to_owned(),
                    Err(e) => format!("复制失败:{e}"),
                };
                false
            }
            KeyCode::Char('H') => {
                let lines = merge.chunks[merge.cursor].ours.clone();
                ui.message = copy_feedback(&lines, "当前块本地侧");
                false
            }
            KeyCode::Char('L') => {
                let lines = merge.chunks[merge.cursor].theirs.clone();
                ui.message = copy_feedback(&lines, "当前块远端侧");
                false
            }
            _ => false,
        },
        FileEntry::Binary { choice, .. } => {
            match code {
                KeyCode::Char('h') | KeyCode::Left => *choice = Some(Side::Ours),
                KeyCode::Char('l') | KeyCode::Right => *choice = Some(Side::Theirs),
                KeyCode::Char('u') => *choice = None,
                _ => {}
            }
            false
        }
    }
}

/// 复制若干行到剪贴板并生成消息条反馈。
fn copy_feedback(lines: &[String], what: &str) -> String {
    match copy_to_clipboard(&lines.join("\n")) {
        Ok(()) => format!("已复制{what}({} 行)", lines.len()),
        Err(e) => format!("复制失败:{e}"),
    }
}

/// 把文本写入系统剪贴板:依次尝试 pbcopy(macOS)/ xclip(X11)/ wl-copy(Wayland)。
fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    const TOOLS: [(&str, &[&str]); 3] = [
        ("pbcopy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("wl-copy", &[]),
    ];
    for (program, args) in TOOLS {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().map(|s| s.success()).unwrap_or(false) {
            return Ok(());
        }
    }
    anyhow::bail!("未找到可用的剪贴板工具(pbcopy / xclip / wl-copy)")
}

/// 写盘当前文件;成功返回 true。
///
/// 冲突块必须全部解决;未处理的非冲突改动会在写盘前**自动应用**,
/// 与 git 自动合并的语义一致(想拒绝某处改动,写盘前用 x 显式忽略)。
/// 否则按 base 写盘会悄悄丢掉 git 已合并进来的对侧改动。
fn write_current(
    session: &mut Session,
    write_file: &mut dyn FnMut(&str, &[u8]) -> Result<()>,
    ui: &mut UiState,
) -> Result<bool> {
    if !session.current_file().ready_to_write() {
        ui.message = "仍有未解决的冲突,无法写入(n 跳到下一处冲突)".to_owned();
        return Ok(false);
    }
    let mut auto_applied = 0;
    if let FileEntry::Text(merge) = session.current_file_mut() {
        auto_applied = merge.pending_changes();
        merge.apply_all_nonconflict();
    }
    let entry = session.current_file();
    let path = entry.path().to_owned();
    write_file(&path, &entry.resolved_bytes())?;
    session.mark_written();
    ui.message = if auto_applied > 0 {
        format!("✔ 已写入 {path}(自动应用了 {auto_applied} 处非冲突改动)")
    } else {
        format!("✔ 已写入 {path}")
    };
    Ok(true)
}

/// 调起 $EDITOR 编辑一段内容;返回 None 表示用户取消(编辑器非零退出)。
fn edit_lines(terminal: &mut DefaultTerminal, initial: &[String]) -> Result<Option<Vec<String>>> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_owned());
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi").to_owned();
    let args: Vec<&str> = parts.collect();

    let path = std::env::temp_dir().join(format!("git-peace-edit-{}.txt", std::process::id()));
    std::fs::write(&path, initial.join("\n"))?;

    // 让出终端给编辑器,结束后重建 TUI
    ratatui::restore();
    let status = std::process::Command::new(&program)
        .args(&args)
        .arg(&path)
        .status();
    *terminal = ratatui::init();
    terminal.clear()?;

    let status = status.with_context(|| format!("启动编辑器 {program} 失败"))?;
    if !status.success() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);
    // 编辑器通常会补一个末尾换行,这里剥掉以免多出空行
    let text = text.strip_suffix('\n').unwrap_or(&text);
    Ok(Some(if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').map(str::to_owned).collect()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::FileMerge;

    /// 回归:写盘时未处理的非冲突改动应自动应用,而非退回 base
    /// (否则会悄悄丢掉 git 已自动合并进来的对侧改动)
    #[test]
    fn write_auto_applies_pending_nonconflict_changes() {
        let merge = FileMerge::from_three_way(
            "demo.txt".to_owned(),
            "a\nb\nc\nd\n",
            "a\nX\nc\nd\n",
            "a\nY\nc\nD\n",
        );
        let mut session = Session::new(vec![FileEntry::Text(merge)], "merge".to_owned());
        // 只解决冲突块(取本地、忽略远端),theirs 侧 d→D 的改动保持未处理
        let FileEntry::Text(m) = session.current_file_mut() else {
            unreachable!()
        };
        m.apply(Side::Ours);
        m.ignore(Side::Theirs);

        let mut written: Vec<u8> = Vec::new();
        let mut ui = UiState::default();
        let ok = write_current(
            &mut session,
            &mut |_path, bytes| {
                written = bytes.to_vec();
                Ok(())
            },
            &mut ui,
        )
        .unwrap();
        assert!(ok);
        assert_eq!(String::from_utf8(written).unwrap(), "a\nX\nc\nD\n");
        assert!(ui.message.contains("自动应用"));
    }
}
