//! 界面整体绘制:状态栏、提示条、消息条、帮助浮层与二进制视图。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{FileEntry, Session, Side};

use super::UiState;
use super::panes::draw_columns;
use super::theme::Theme;

/// 绘制整个界面(独立成 pub 函数,便于 TestBackend 冒烟测试)。
///
/// 需要可变会话:绘制时会把滚动位置写回当前文件,跨帧保持视口稳定;
/// 需要可变 UI 状态:高亮缓存按需构建。
pub fn draw(frame: &mut Frame, session: &mut Session, ui: &mut UiState) {
    let [header, body, hints, message] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, session, &ui.theme);
    let folded = session.folded;
    let file_idx = session.current;
    match session.current_file_mut() {
        FileEntry::Text(merge) => {
            let highlight = ui.cache.get(file_idx, merge, ui.revision, ui.theme.light);
            draw_columns(frame, body, merge, folded, &ui.theme, highlight);
        }
        FileEntry::Binary {
            path,
            ours,
            theirs,
            choice,
        } => draw_binary(frame, body, path, ours, theirs, *choice, &ui.theme),
    }
    draw_hints(frame, hints, &ui.theme);
    frame.render_widget(
        Paragraph::new(ui.message.as_str()).style(Style::new().fg(ui.theme.amber)),
        message,
    );
    if ui.show_help {
        draw_help(frame, &ui.theme);
    }
}

/// 顶部状态条:操作徽章 · 进度条 · 文件路径 · 冲突计数。
fn draw_header(frame: &mut Frame, area: Rect, session: &Session, theme: &Theme) {
    let entry = session.current_file();
    let pending = match entry {
        FileEntry::Text(m) => m.pending_conflicts(),
        FileEntry::Binary { choice, .. } => usize::from(choice.is_none()),
    };
    let done = session.written.iter().filter(|&&w| w).count();
    let total = session.files.len();

    // █░ 进度条:已写盘文件数 / 总文件数
    const BAR_WIDTH: usize = 10;
    let filled = (done * BAR_WIDTH).checked_div(total).unwrap_or(0);
    let sep = Span::styled("▏", Style::new().fg(theme.border));

    let mut spans = vec![
        Span::styled(
            format!(" ⚑ {} ", session.op_label.to_uppercase()),
            Style::new()
                .fg(theme.badge_fg)
                .bg(theme.blue)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("█".repeat(filled), Style::new().fg(theme.blue)),
        Span::styled(
            "░".repeat(BAR_WIDTH - filled),
            Style::new().fg(theme.border),
        ),
        Span::styled(
            format!(" {}/{} 文件 ", session.current + 1, total),
            Style::new().fg(theme.fg_bright),
        ),
        sep.clone(),
        Span::raw(format!(" {} ", entry.path())),
        sep,
    ];
    if pending > 0 {
        spans.push(Span::styled(
            format!(" ✗ {pending} 冲突待解决"),
            Style::new().fg(theme.red).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            " ✓ 本文件已就绪(w 写盘)",
            Style::new().fg(theme.green),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 二进制文件的降级视图:LOCAL / REMOTE 两张卡片整文件二选一。
fn draw_binary(
    frame: &mut Frame,
    area: Rect,
    path: &str,
    ours: &[u8],
    theirs: &[u8],
    choice: Option<Side>,
    theme: &Theme,
) {
    let panel = centered_rect(area, 64, 11);
    let [notice, _, cards, _, hint] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(panel);

    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(path.to_owned(), Style::new().add_modifier(Modifier::BOLD)).centered(),
            Line::styled(
                "二进制文件无法逐块合并,请整体选择一侧",
                Style::new().fg(theme.amber),
            )
            .centered(),
        ]),
        notice,
    );

    let [left, _, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(cards);
    binary_card(
        frame,
        left,
        " LOCAL ",
        "h",
        ours,
        choice == Some(Side::Ours),
        theme,
    );
    binary_card(
        frame,
        right,
        " REMOTE ",
        "l",
        theirs,
        choice == Some(Side::Theirs),
        theme,
    );

    frame.render_widget(
        Paragraph::new(Line::styled(
            "h / l 选择 · u 撤销 · 选择后 w 写盘",
            Style::new().fg(theme.fg_dim),
        ))
        .centered(),
        hint,
    );
}

/// 单张二进制候选卡片:选中侧边框换绿色并在标题加 ✓。
fn binary_card(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    key: &str,
    bytes: &[u8],
    selected: bool,
    theme: &Theme,
) {
    let (border, title_text) = if selected {
        (theme.green, format!("{title}✓ "))
    } else {
        (theme.border, title.to_owned())
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .title(Span::styled(
            title_text,
            Style::new()
                .fg(theme.fg_bright)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!("{} 字节", bytes.len()),
                Style::new().fg(theme.fg_bright),
            )
            .centered(),
            Line::styled(format!("按 {key} 选择"), Style::new().fg(theme.fg_dim)).centered(),
        ]),
        inner,
    );
}

/// 键帽样式的按键 Span(底色微凸起)。
fn keycap(key: &str, theme: &Theme) -> Span<'static> {
    Span::styled(
        format!(" {key} "),
        Style::new().fg(theme.keycap_fg).bg(theme.keycap_bg),
    )
}

/// 底部按键提示条(键帽 + 描述)。
fn draw_hints(frame: &mut Frame, area: Rect, theme: &Theme) {
    const ITEMS: [(&str, &str); 11] = [
        ("h", "取左"),
        ("l", "取右"),
        ("x", "忽略"),
        ("u/U", "撤销"),
        ("e", "编辑"),
        ("n/p", "冲突"),
        ("w", "写盘"),
        ("⇥", "文件"),
        ("z", "折叠"),
        ("q", "退出"),
        ("?", "帮助"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (key, desc) in ITEMS {
        spans.push(keycap(key, theme));
        spans.push(Span::styled(
            format!(" {desc}  "),
            Style::new().fg(theme.hint_fg),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 帮助浮层:圆角边框 + 双栏按键布局。
fn draw_help(frame: &mut Frame, theme: &Theme) {
    const LEFT: [(&str, &str); 8] = [
        ("h / ←", "取用本地(两侧先后取用=都要)"),
        ("l / →", "取用远端改动"),
        ("x", "忽略未处理的侧(保留 base)"),
        ("u / U", "撤销当前块 / 全部块"),
        ("e", "$EDITOR 编辑当前块"),
        ("a", "应用全部非冲突改动"),
        ("w", "写盘(自动应用非冲突)"),
        ("q", "退出(现场保留)"),
    ];
    const RIGHT: [(&str, &str); 8] = [
        ("j / k", "上下移动改动块"),
        ("n / p", "下/上一个未解决冲突"),
        ("Tab", "切换到下一个文件"),
        ("z", "折叠/展开未改动区域"),
        ("y", "复制当前块结果"),
        ("Y", "复制整个文件结果"),
        ("H / L", "复制块的本地/远端侧"),
        ("?", "打开本帮助"),
    ];
    let column = |entries: &[(&str, &str)]| -> Vec<Line<'static>> {
        entries
            .iter()
            .map(|(key, desc)| {
                Line::from(vec![
                    Span::styled(
                        format!(" {key:<7}"),
                        Style::new().fg(theme.blue).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw((*desc).to_owned()),
                ])
            })
            .collect()
    };

    let area = centered_rect(frame.area(), 78, 10);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .title(Span::styled(
            " 按键说明 ",
            Style::new()
                .fg(theme.fg_bright)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                " 按任意键关闭 ",
                Style::new().fg(theme.fg_dim),
            ))
            .centered(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
    frame.render_widget(Paragraph::new(column(&LEFT)), left);
    frame.render_widget(Paragraph::new(column(&RIGHT)), right);
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
    use crate::app::FileMerge;
    use crate::ui::rows::build_rows;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// 用 TestBackend 渲染一帧,断言关键内容存在且不 panic
    #[test]
    fn draw_smoke_renders_key_elements() {
        let merge =
            FileMerge::from_three_way("demo.txt".to_owned(), "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        let mut session = Session::new(vec![FileEntry::Text(merge)], "merge".to_owned());
        let mut ui = UiState::default();

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut session, &mut ui))
            .unwrap();

        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("demo.txt"));
        // 状态栏操作徽章与进度条
        assert!(content.contains("MERGE"));
        assert!(content.contains("░"));
        // 圆角边框面板与嵌入边框的栏头
        assert!(content.contains("╭"));
        assert!(content.contains("LOCAL"));
        assert!(content.contains("RESULT"));
        assert!(content.contains("REMOTE"));
        // 未解决冲突:结果栏占位带 + 两侧 gutter 取用箭头
        assert!(content.contains("待解决"));
        assert!(content.contains("»"));
        assert!(content.contains("«"));
    }

    /// 窄终端(60 列)渲染不 panic,预算逐级降级
    #[test]
    fn draw_narrow_terminal_smoke() {
        let merge =
            FileMerge::from_three_way("demo.txt".to_owned(), "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        let mut session = Session::new(vec![FileEntry::Text(merge)], "merge".to_owned());
        let mut ui = UiState::default();
        for width in [60u16, 30, 12] {
            let backend = TestBackend::new(width, 16);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw(frame, &mut session, &mut ui))
                .unwrap();
        }
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
        let mut ui = UiState::default();
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        // 视口正文高度 = 24 - 状态条/提示/消息各 1 行 - 上下边框 2 行
        let body_height = 19usize;

        // 跳到最后一个冲突,再跳回中间的冲突
        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        for step in ["next", "next", "prev"] {
            let FileEntry::Text(m) = session.current_file_mut() else {
                unreachable!()
            };
            if step == "next" {
                m.next_conflict();
            } else {
                m.prev_conflict();
            }
            terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
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
            .draw(|frame| draw(frame, &mut session, &mut UiState::default()))
            .unwrap();
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("logo.png"));
    }
}
