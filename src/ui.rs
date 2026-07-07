//! ratatui three-pane rendering and the key-event loop.
//!
//! Layout: status bar / column titles / three-column body (local | result | remote)
//! / key hints / message line. Change chunks are tinted as full-width bands
//! (blue = one-sided, green = agreement, red = conflict); the band disappears
//! once a chunk is resolved, the current chunk is highlighted, and `?` shows
//! the full key reference.

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::app::{FileEntry, FileMerge, Session, Side};
use crate::merge::ChunkKind;

/// 稳定块折叠阈值与首尾保留行数(与 Web 版一致)。
const FOLD_THRESHOLD: usize = 8;
const FOLD_KEEP: usize = 3;

// —— 配色(Tokyo Night 系,暗色终端调校)——
// 选中提亮遵循「同色相加深增饱和」而非均匀加灰,避免颜色发浑

/// 次要文字 / 行号
const FG_DIM: Color = Color::Rgb(108, 116, 130);
/// 选中块的行号
const FG_BRIGHT: Color = Color::Rgb(205, 214, 244);
/// 蓝(单侧改动)
const ACCENT_BLUE: Color = Color::Rgb(122, 162, 247);
/// 绿(一致改动 / 就绪)
const ACCENT_GREEN: Color = Color::Rgb(158, 206, 106);
/// 红(冲突)
const ACCENT_RED: Color = Color::Rgb(224, 108, 117);
/// 琥珀(消息与提示)
const ACCENT_AMBER: Color = Color::Rgb(229, 192, 123);

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
}

/// 运行交互会话直至完成或退出。
///
/// `write_file` 负责把解决后的字节落盘(git 模式下还会顺带 `git add`)。
pub fn run_session(
    session: &mut Session,
    write_file: &mut dyn FnMut(&str, &[u8]) -> Result<()>,
) -> Result<Outcome> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, session, write_file);
    ratatui::restore();
    result
}

/// 事件主循环:绘制 → 读键 → 更新状态。
fn event_loop(
    terminal: &mut DefaultTerminal,
    session: &mut Session,
    write_file: &mut dyn FnMut(&str, &[u8]) -> Result<()>,
) -> Result<Outcome> {
    let mut ui = UiState::default();
    loop {
        terminal.draw(|frame| draw(frame, session, &ui))?;
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
                if write_current(session, write_file, &mut ui)? && session.all_written() {
                    return Ok(Outcome::Completed);
                }
            }
            KeyCode::Char('e') => {
                if let FileEntry::Text(merge) = session.current_file_mut() {
                    let initial = merge.current_content(merge.cursor);
                    if let Some(lines) = edit_lines(terminal, &initial)? {
                        merge.set_override(lines);
                        ui.message = "已用编辑结果覆写当前块".to_owned();
                    } else {
                        ui.message = "编辑已取消".to_owned();
                    }
                }
            }
            code => handle_file_key(session, code, &mut ui),
        }
    }
}

/// 处理作用于当前文件的普通按键。
fn handle_file_key(session: &mut Session, code: KeyCode, ui: &mut UiState) {
    match session.current_file_mut() {
        FileEntry::Text(merge) => match code {
            KeyCode::Char('h') | KeyCode::Left => merge.apply(Side::Ours),
            KeyCode::Char('l') | KeyCode::Right => merge.apply(Side::Theirs),
            // x = 忽略当前块所有仍待处理的侧(已取用的内容保留)
            KeyCode::Char('x') => {
                merge.ignore(Side::Ours);
                merge.ignore(Side::Theirs);
            }
            KeyCode::Char('u') => merge.undo(),
            KeyCode::Char('a') => {
                merge.apply_all_nonconflict();
                ui.message = "已应用全部非冲突改动".to_owned();
            }
            KeyCode::Char('j') | KeyCode::Down => merge.next_change(),
            KeyCode::Char('k') | KeyCode::Up => merge.prev_change(),
            KeyCode::Char('n') => merge.next_conflict(),
            KeyCode::Char('p') => merge.prev_conflict(),
            // 复制键(终端框选会横跨三栏,复制键绕开这个限制):
            // y 块结果 / Y 整个文件结果 / H 块本地侧 / L 块远端侧
            KeyCode::Char('y') => {
                let lines = merge.current_content(merge.cursor);
                ui.message = copy_feedback(&lines, "当前块结果");
            }
            KeyCode::Char('Y') => {
                ui.message = match copy_to_clipboard(&merge.resolved_content()) {
                    Ok(()) => "已复制整个文件的当前结果".to_owned(),
                    Err(e) => format!("复制失败:{e}"),
                };
            }
            KeyCode::Char('H') => {
                let lines = merge.chunks[merge.cursor].ours.clone();
                ui.message = copy_feedback(&lines, "当前块本地侧");
            }
            KeyCode::Char('L') => {
                let lines = merge.chunks[merge.cursor].theirs.clone();
                ui.message = copy_feedback(&lines, "当前块远端侧");
            }
            _ => {}
        },
        FileEntry::Binary { choice, .. } => match code {
            KeyCode::Char('h') | KeyCode::Left => *choice = Some(Side::Ours),
            KeyCode::Char('l') | KeyCode::Right => *choice = Some(Side::Theirs),
            KeyCode::Char('u') => *choice = None,
            _ => {}
        },
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

// —— 渲染 ——

/// 一个渲染行:三栏各自的(行号, 文本),或跨三栏的折叠提示。
struct Row {
    kind: ChunkKind,
    resolved: bool,
    current: bool,
    fold: Option<String>,
    ours: Option<(usize, String)>,
    result: Option<(usize, String)>,
    theirs: Option<(usize, String)>,
}

/// 把文件的块序列展开为渲染行,并返回每块首行的行下标(滚动定位用)。
fn build_rows(merge: &FileMerge, folded: bool) -> (Vec<Row>, Vec<usize>) {
    let mut rows: Vec<Row> = Vec::new();
    let mut chunk_starts: Vec<usize> = Vec::new();
    let mut result_no = 1usize;

    for (idx, chunk) in merge.chunks.iter().enumerate() {
        chunk_starts.push(rows.len());
        let resolved = merge.chunk_resolved(idx);
        let current = idx == merge.cursor && chunk.kind != ChunkKind::Stable;
        let result_lines = merge.current_content(idx);

        // 生成一段三栏平行行;off 为段内偏移
        let push_slice = |rows: &mut Vec<Row>, range: std::ops::Range<usize>| {
            for i in range {
                rows.push(Row {
                    kind: chunk.kind,
                    resolved,
                    current,
                    fold: None,
                    ours: chunk.ours.get(i).map(|t| (chunk.ours_start + i, t.clone())),
                    result: result_lines.get(i).map(|t| (result_no + i, t.clone())),
                    theirs: chunk
                        .theirs
                        .get(i)
                        .map(|t| (chunk.theirs_start + i, t.clone())),
                });
            }
        };

        let len = chunk.base.len();
        if chunk.kind == ChunkKind::Stable && folded && len > FOLD_THRESHOLD {
            push_slice(&mut rows, 0..FOLD_KEEP);
            rows.push(Row {
                kind: chunk.kind,
                resolved,
                current: false,
                fold: Some(format!("⋯ {} 行未改动(z 展开)⋯", len - FOLD_KEEP * 2)),
                ours: None,
                result: None,
                theirs: None,
            });
            push_slice(&mut rows, len - FOLD_KEEP..len);
        } else {
            let height = chunk
                .ours
                .len()
                .max(result_lines.len())
                .max(chunk.theirs.len())
                .max(1);
            push_slice(&mut rows, 0..height);
        }
        result_no += result_lines.len();
    }
    (rows, chunk_starts)
}

/// 各块类型的色带背景;(普通, 选中)成对手工调校,选中版同色相加深提亮。
fn band_bg(kind: ChunkKind, current: bool) -> Option<Color> {
    let (normal, selected) = match kind {
        ChunkKind::Ours | ChunkKind::Theirs => ((28, 39, 58), (45, 64, 96)),
        ChunkKind::Agree => ((26, 42, 31), (40, 66, 48)),
        ChunkKind::Conflict => ((58, 30, 34), (94, 45, 53)),
        ChunkKind::Stable => return None,
    };
    let (r, g, b) = if current { selected } else { normal };
    Some(Color::Rgb(r, g, b))
}

/// 各块类型的强调色(选中指示条使用,与色带同色相)。
fn kind_accent(kind: ChunkKind) -> Color {
    match kind {
        ChunkKind::Ours | ChunkKind::Theirs => ACCENT_BLUE,
        ChunkKind::Agree => ACCENT_GREEN,
        ChunkKind::Conflict => ACCENT_RED,
        ChunkKind::Stable => FG_DIM,
    }
}

/// 组装一栏中的一行文本(选中指示条 + 行号 + 内容),cell 为 None 时输出空白。
fn cell_line(row: &Row, cell: &Option<(usize, String)>) -> Line<'static> {
    if let Some(text) = &row.fold {
        return Line::styled(format!(" {text}"), Style::new().fg(FG_DIM));
    }
    let mut style = Style::new();
    if !row.resolved
        && let Some(bg) = band_bg(row.kind, row.current)
    {
        style = style.bg(bg);
    }
    // 当前块以「同色相指示条 + 提亮色带 + 亮行号」标识,颜色彼此同族不撞色
    let marker = if row.current {
        Span::styled("▌", Style::new().fg(kind_accent(row.kind)))
    } else {
        Span::raw(" ")
    };
    let Some((no, text)) = cell else {
        return Line::from(vec![marker]).style(style);
    };
    let no_style = if row.current {
        Style::new().fg(FG_BRIGHT)
    } else {
        Style::new().fg(FG_DIM)
    };
    Line::from(vec![
        marker,
        Span::styled(format!("{no:>4} "), no_style),
        Span::raw(text.clone()),
    ])
    .style(style)
}

/// 绘制整个界面(独立成 pub 函数,便于 TestBackend 冒烟测试)。
///
/// 需要可变会话:绘制时会把滚动位置写回当前文件,跨帧保持视口稳定。
pub fn draw(frame: &mut Frame, session: &mut Session, ui: &UiState) {
    let [header, titles, body, hints, message] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, session);
    let folded = session.folded;
    match session.current_file_mut() {
        FileEntry::Text(merge) => {
            draw_titles(frame, titles, merge);
            draw_columns(frame, body, merge, folded);
        }
        FileEntry::Binary {
            path,
            ours,
            theirs,
            choice,
        } => draw_binary(frame, body, path, ours, theirs, *choice),
    }
    draw_hints(frame, hints);
    frame.render_widget(
        Paragraph::new(ui.message.as_str()).style(Style::new().fg(ACCENT_AMBER)),
        message,
    );
    if ui.show_help {
        draw_help(frame);
    }
}

/// 顶部状态条:操作 · 文件进度 · 冲突计数。
fn draw_header(frame: &mut Frame, area: Rect, session: &Session) {
    let entry = session.current_file();
    let pending = match entry {
        FileEntry::Text(m) => m.pending_conflicts(),
        FileEntry::Binary { choice, .. } => usize::from(choice.is_none()),
    };
    let done = session.written.iter().filter(|&&w| w).count();
    let mut spans = vec![
        Span::styled(
            format!(" git-peace · {} ", session.op_label),
            Style::new().fg(Color::Rgb(16, 18, 24)).bg(ACCENT_BLUE),
        ),
        Span::raw(format!(
            " 文件 {}/{}({} 已完成) · {} ",
            session.current + 1,
            session.files.len(),
            done,
            entry.path()
        )),
    ];
    if pending > 0 {
        spans.push(Span::styled(
            format!("待解决冲突 {pending}"),
            Style::new().fg(ACCENT_RED).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            "✔ 本文件已就绪(w 写盘)",
            Style::new().fg(ACCENT_GREEN),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 三栏栏头(带分支标签)。
fn draw_titles(frame: &mut Frame, area: Rect, merge: &FileMerge) {
    let cols = split_columns(area);
    let ours = merge
        .ours_label
        .as_deref()
        .map_or("本地 (ours)".to_owned(), |l| format!("本地 ({l})"));
    let theirs = merge
        .theirs_label
        .as_deref()
        .map_or("远端 (theirs)".to_owned(), |l| format!("远端 ({l})"));
    let style = Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD);
    frame.render_widget(Paragraph::new(ours).style(style), cols[0]);
    frame.render_widget(Paragraph::new("结果 (result)").style(style), cols[1]);
    frame.render_widget(Paragraph::new(theirs).style(style), cols[2]);
}

/// 三列正文;根据光标调整滚动位置并写回,跨帧保持视口稳定。
fn draw_columns(frame: &mut Frame, area: Rect, merge: &mut FileMerge, folded: bool) {
    let (rows, chunk_starts) = build_rows(merge, folded);
    let height = area.height as usize;
    let max_scroll = rows.len().saturating_sub(height);

    // 光标块在视口内则不动;跳出视口时把块首行定位到约 1/3 处,
    // 让上方留出上下文、下方能看到块的内容
    let target = chunk_starts.get(merge.cursor).copied().unwrap_or(0);
    let mut scroll = merge.scroll.min(max_scroll);
    if height > 0 && (target < scroll || target >= scroll + height) {
        scroll = target.saturating_sub(height / 3).min(max_scroll);
    }
    merge.scroll = scroll;

    let visible = &rows[scroll.min(rows.len())..rows.len().min(scroll + height)];
    let cols = split_columns(area);
    let build = |pick: fn(&Row) -> &Option<(usize, String)>| -> Vec<Line<'static>> {
        visible
            .iter()
            .map(|row| cell_line(row, pick(row)))
            .collect()
    };
    frame.render_widget(Paragraph::new(build(|r| &r.ours)), cols[0]);
    frame.render_widget(Paragraph::new(build(|r| &r.result)), cols[1]);
    frame.render_widget(Paragraph::new(build(|r| &r.theirs)), cols[2]);
}

/// 二进制文件的降级视图:整文件二选一。
fn draw_binary(
    frame: &mut Frame,
    area: Rect,
    path: &str,
    ours: &[u8],
    theirs: &[u8],
    choice: Option<Side>,
) {
    let chosen = match choice {
        Some(Side::Ours) => "当前选择:本地版本",
        Some(Side::Theirs) => "当前选择:远端版本",
        None => "尚未选择",
    };
    let text = vec![
        Line::raw(""),
        Line::styled(
            format!("{path} 是二进制文件,无法逐块合并"),
            Style::new().fg(ACCENT_AMBER),
        ),
        Line::raw(""),
        Line::raw(format!("  h 整体取本地({} 字节)", ours.len())),
        Line::raw(format!("  l 整体取远端({} 字节)", theirs.len())),
        Line::raw(""),
        Line::styled(chosen, Style::new().add_modifier(Modifier::BOLD)),
        Line::raw(""),
        Line::styled("选择后按 w 写盘", Style::new().fg(FG_DIM)),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

/// 底部按键提示条。
fn draw_hints(frame: &mut Frame, area: Rect) {
    let hints = " h 取本地 · l 取远端 · x 忽略 · u 撤销 · e 编辑 · a 全部非冲突 · y 复制 · j/k 块 · n/p 冲突 · w 写盘 · Tab 换文件 · z 折叠 · q 退出 · ? 帮助";
    frame.render_widget(Paragraph::new(hints).style(Style::new().fg(FG_DIM)), area);
}

/// 帮助浮层。
fn draw_help(frame: &mut Frame) {
    let area = centered_rect(frame.area(), 56, 21);
    let lines = vec![
        Line::styled("按键说明", Style::new().add_modifier(Modifier::BOLD)),
        Line::raw(""),
        Line::raw("h / ←     取用本地改动(冲突两侧先后取用 = 两者都要)"),
        Line::raw("l / →     取用远端改动"),
        Line::raw("x         忽略当前块剩余未处理的侧(保留 base)"),
        Line::raw("u         撤销当前块的全部决定"),
        Line::raw("e         用 $EDITOR 手动编辑当前块"),
        Line::raw("a         一键应用所有非冲突改动"),
        Line::raw("y / Y     复制当前块结果 / 整个文件结果到剪贴板"),
        Line::raw("H / L     复制当前块的本地侧 / 远端侧内容"),
        Line::raw("j / k     上下移动改动块"),
        Line::raw("n / p     跳到下/上一个未解决冲突"),
        Line::raw("w         写盘当前文件(自动应用剩余非冲突改动 + git add)"),
        Line::raw("Tab       切换到下一个文件"),
        Line::raw("z         展开 / 折叠未改动区域"),
        Line::raw("q         退出(现场保留,可随时回来继续)"),
        Line::raw(""),
        Line::styled("按任意键关闭", Style::new().fg(FG_DIM)),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" 帮助 ")),
        area,
    );
}

/// 将区域水平三等分。
fn split_columns(area: Rect) -> [Rect; 3] {
    Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .areas(area)
}

/// 居中的浮层矩形(不超出屏幕)。
fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::FileEntry;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// 用 TestBackend 渲染一帧,断言关键内容存在且不 panic
    #[test]
    fn draw_smoke_renders_key_elements() {
        let merge =
            FileMerge::from_three_way("demo.txt".to_owned(), "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        let mut session = Session::new(vec![FileEntry::Text(merge)], "merge".to_owned());
        let ui = UiState::default();

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut session, &ui))
            .unwrap();

        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("demo.txt"));
        assert!(content.contains("git-peace"));
    }

    /// 折叠行与行号构建正确
    #[test]
    fn build_rows_folds_long_stable_chunks() {
        let base: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let merge = FileMerge::from_three_way("demo.txt".to_owned(), &base, &base, &base);
        let (rows, starts) = build_rows(&merge, true);
        // 20 行稳定区折叠为 3 + 折叠行 + 3
        assert_eq!(rows.len(), 7);
        assert!(rows[3].fold.is_some());
        assert_eq!(starts, vec![0]);

        let (unfolded, _) = build_rows(&merge, false);
        assert_eq!(unfolded.len(), 20);
    }

    /// 回归:跳转冲突后选中块应留在视口内且不贴底(依赖 scroll 跨帧持久化)
    #[test]
    fn scroll_keeps_selected_chunk_in_view() {
        // 三个冲突,彼此隔着 40 行稳定区;展开渲染保证行数远超视口
        let stable: String = (1..=40).map(|i| format!("s{i}\n")).collect();
        let base = format!("a\n{stable}b\n{stable}c\n");
        let ours = format!("A\n{stable}B\n{stable}C\n");
        let theirs = format!("X\n{stable}Y\n{stable}Z\n");
        let merge = FileMerge::from_three_way("demo.txt".to_owned(), &base, &ours, &theirs);
        let mut session = Session::new(vec![FileEntry::Text(merge)], "merge".to_owned());
        session.folded = false;
        let ui = UiState::default();
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        // 视口正文高度 = 24 - 状态条/栏头/提示/消息各 1 行
        let body_height = 20usize;

        // 跳到最后一个冲突,再跳回中间的冲突
        terminal.draw(|f| draw(f, &mut session, &ui)).unwrap();
        for step in ["next", "next", "prev"] {
            let FileEntry::Text(m) = session.current_file_mut() else {
                unreachable!()
            };
            if step == "next" {
                m.next_conflict();
            } else {
                m.prev_conflict();
            }
            terminal.draw(|f| draw(f, &mut session, &ui)).unwrap();
        }

        let FileEntry::Text(m) = session.current_file() else {
            unreachable!()
        };
        let (_, starts) = build_rows(m, false);
        let target = starts[m.cursor];
        assert!(target >= m.scroll, "选中块滚出了视口上方");
        assert!(target < m.scroll + body_height, "选中块滚出了视口下方");
        // 不贴底:块首行下方至少还能看到两行块内容
        assert!(
            target + 2 < m.scroll + body_height,
            "选中块贴在视口底部(target={target}, scroll={})",
            m.scroll
        );
    }

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

    /// 二进制条目渲染不 panic
    #[test]
    fn draw_binary_entry_smoke() {
        let mut session = Session::new(
            vec![FileEntry::Binary {
                path: "logo.png".to_owned(),
                ours: vec![0, 1],
                theirs: vec![2, 3],
                choice: None,
            }],
            "merge".to_owned(),
        );
        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut session, &UiState::default()))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("logo.png"));
    }
}
