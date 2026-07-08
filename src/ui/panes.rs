//! 三栏正文渲染:圆角边框面板、色带、词级强调、选中指示、行号、折叠横线与滚动条。

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::app::{FileMerge, SideState};

use super::highlight::{FileHighlight, PaneSyntax};
use super::rows::{Cell, Row, build_rows};
use super::theme::Theme;

/// 三栏之一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    /// 本地(左栏, ours)
    Local,
    /// 合并结果(中栏)
    Result,
    /// 远端(右栏, theirs)
    Remote,
}

/// 一栏内的空间预算:窄终端时逐级裁剪次要元素(先丢行号,再丢 gutter)。
struct CellBudget {
    /// 是否显示左侧 gutter 列(操作符号 + 空格)
    gutter: bool,
    /// 行号列宽;0 表示不显示行号
    no_width: usize,
}

/// 依据栏内宽与最大行号计算空间预算。
fn cell_budget(inner: u16, max_no: usize) -> CellBudget {
    let no_width = max_no.to_string().len().max(3);
    if inner >= 24 {
        CellBudget {
            gutter: true,
            no_width,
        }
    } else if inner >= 14 {
        CellBudget {
            gutter: true,
            no_width: 0,
        }
    } else {
        CellBudget {
            gutter: false,
            no_width: 0,
        }
    }
}

/// gutter 操作符号(IDEA 风格):
/// `»` / `«` 待取用(箭头指向结果栏)、`✓` 已取用、`✗` 已忽略。
/// 结果栏不放符号(紧贴边框的色块会被误读为边框变色),
/// 当前块由提亮色带 + 亮行号标识。
fn gutter_symbol(row: &Row, pane: Pane, theme: &Theme) -> Span<'static> {
    let sym = |state: Option<SideState>, arrow: &'static str| match state {
        // 覆写(e 编辑)后块已解决,不再提示待取用
        Some(SideState::Pending) if !row.resolved => {
            Span::styled(arrow, Style::new().fg(theme.accent(row.change)))
        }
        Some(SideState::Applied) => Span::styled("✓", Style::new().fg(theme.green)),
        Some(SideState::Ignored) => Span::styled("✗", Style::new().fg(theme.fg_dim)),
        _ => Span::raw(" "),
    };
    match pane {
        Pane::Local => sym(row.ours_state, "»"),
        Pane::Remote => sym(row.theirs_state, "«"),
        Pane::Result => Span::raw(" "),
    }
}

/// 栏头标题(嵌入边框):固定栏名 + 分支标签。
fn pane_title(pane: Pane, merge: &FileMerge) -> String {
    match pane {
        Pane::Local => merge
            .ours_label
            .as_deref()
            .map_or(" LOCAL (ours) ".to_owned(), |l| format!(" LOCAL · {l} ")),
        Pane::Result => " RESULT ".to_owned(),
        Pane::Remote => merge
            .theirs_label
            .as_deref()
            .map_or(" REMOTE (theirs) ".to_owned(), |l| {
                format!(" REMOTE · {l} ")
            }),
    }
}

/// 折叠行:左右栏画纯横线,中栏在横线中嵌入行数标签(窄栏逐级降级)。
/// 横线与面板边框同色,保证全部线条元素色调一致。
fn fold_line(n: usize, pane: Pane, inner: u16, theme: &Theme) -> Line<'static> {
    let style = Style::new().fg(theme.border);
    if pane != Pane::Result {
        return Line::styled("─".repeat(inner as usize), style);
    }
    // 中栏标签按可用宽度降级:完整文案 → 精简 → 纯横线
    let text = if inner >= 30 {
        format!("── ⋯ {n} 行未改动 (z 展开) ⋯ ──")
    } else if inner >= 12 {
        format!("─ ⋯ {n} ⋯ ─")
    } else {
        "─".repeat(inner as usize)
    };
    Line::styled(text, style).alignment(Alignment::Center)
}

/// 三层样式合成:按 fg(语法高亮)与 emphasis(词级强调)的区间端点分段,
/// 每段 fg 取语法色、命中 emphasis 处 bg 换更亮的强调色;色带 bg 由行样式提供。
fn compose_spans(
    text: &str,
    fg_spans: &[(Color, Range<usize>)],
    emphasis: &[Range<usize>],
    emph_bg: Color,
) -> Vec<Span<'static>> {
    let mut bounds = vec![0, text.len()];
    for r in fg_spans.iter().map(|(_, r)| r).chain(emphasis) {
        bounds.push(r.start.min(text.len()));
        bounds.push(r.end.min(text.len()));
    }
    bounds.sort_unstable();
    bounds.dedup();
    let mut spans = Vec::with_capacity(bounds.len());
    for win in bounds.windows(2) {
        let (a, b) = (win[0], win[1]);
        let mut style = Style::new();
        if let Some((color, _)) = fg_spans.iter().find(|(_, r)| r.start <= a && b <= r.end) {
            style = style.fg(*color);
        }
        if emphasis.iter().any(|r| r.start <= a && b <= r.end) {
            style = style.bg(emph_bg);
        }
        spans.push(Span::styled(text[a..b].to_owned(), style));
    }
    spans
}

/// 组装一栏中的一行(gutter 符号 + 行号 + 内容 / 占位 / 空白)。
fn cell_line(
    row: &Row,
    cell: &Cell,
    pane: Pane,
    budget: &CellBudget,
    theme: &Theme,
    emphasis: &[Range<usize>],
    fg_spans: &[(Color, Range<usize>)],
) -> Line<'static> {
    let mut style = Style::new();
    // 色带只画在发生改动的栏 + 结果栏(IDEA 式),随块解决消失;当前块同色相提亮
    let banded = match pane {
        Pane::Local => row.ours_state.is_some(),
        Pane::Remote => row.theirs_state.is_some(),
        Pane::Result => true,
    };
    if banded
        && !row.resolved
        && let Some(bg) = theme.band_bg(row.change, row.current)
    {
        style = style.bg(bg);
    }
    let mut spans = Vec::with_capacity(4);
    if budget.gutter {
        spans.push(gutter_symbol(row, pane, theme));
        spans.push(Span::raw(" "));
    }
    let (no, text) = match cell {
        Cell::Line { no, text, .. } => (*no, text),
        Cell::Empty => return Line::from(spans).style(style),
        Cell::Placeholder => {
            spans.push(Span::styled(
                "⋯ 待解决 ⋯",
                Style::new().fg(theme.placeholder_fg),
            ));
            return Line::from(spans).style(style);
        }
    };
    if budget.no_width > 0 {
        let no_style = if row.current {
            Style::new().fg(theme.fg_bright)
        } else {
            Style::new().fg(theme.fg_dim)
        };
        spans.push(Span::styled(
            format!("{no:>w$} ", w = budget.no_width),
            no_style,
        ));
    }
    if emphasis.is_empty() && fg_spans.is_empty() {
        spans.push(Span::raw(text.clone()));
    } else {
        spans.extend(compose_spans(
            text,
            fg_spans,
            emphasis,
            theme.emph_bg(row.change, row.current),
        ));
    }
    Line::from(spans).style(style)
}

/// 取某栏某行的语法高亮段;无语法定义或行号越界时为空。
fn pane_fg(pane: Option<&PaneSyntax>, no: usize) -> &[(Color, Range<usize>)] {
    pane.and_then(|p| p.lines.get(no.wrapping_sub(1)))
        .map_or(&[], Vec::as_slice)
}

/// 三列正文;根据光标调整滚动位置并写回,跨帧保持视口稳定。
pub(crate) fn draw_columns(
    frame: &mut Frame,
    area: Rect,
    merge: &mut FileMerge,
    folded: bool,
    theme: &Theme,
    highlight: &FileHighlight,
) {
    let (rows, chunk_starts) = build_rows(merge, folded);
    // 上下边框各占一行
    let height = (area.height as usize).saturating_sub(2);
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
    // 行号列宽按全文最大行号计算,滚动时保持稳定
    let max_no = rows
        .iter()
        .flat_map(|r| [&r.ours, &r.result, &r.theirs])
        .filter_map(|c| match c {
            Cell::Line { no, .. } => Some(*no),
            _ => None,
        })
        .max()
        .unwrap_or(1);

    let cols = split_columns(area);
    let panes = [
        (Pane::Local, cols[0]),
        (Pane::Result, cols[1]),
        (Pane::Remote, cols[2]),
    ];
    for (pane, col) in panes {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.border))
            .title(Span::styled(
                pane_title(pane, merge),
                Style::new()
                    .fg(theme.fg_bright)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(col);
        let budget = cell_budget(inner.width, max_no);
        let lines: Vec<Line<'static>> = visible
            .iter()
            .map(|row| {
                let cell = match pane {
                    Pane::Local => &row.ours,
                    Pane::Result => &row.result,
                    Pane::Remote => &row.theirs,
                };
                // 词级强调只作用于未解决块的左右两栏(结果栏展示的是已选内容)
                let emphasis: &[Range<usize>] = match (row.resolved, pane, cell) {
                    (false, Pane::Local, Cell::Line { offset, .. }) => highlight
                        .emphasis
                        .get(row.chunk)
                        .and_then(|e| e.ours.get(*offset))
                        .map_or(&[], Vec::as_slice),
                    (false, Pane::Remote, Cell::Line { offset, .. }) => highlight
                        .emphasis
                        .get(row.chunk)
                        .and_then(|e| e.theirs.get(*offset))
                        .map_or(&[], Vec::as_slice),
                    _ => &[],
                };
                // 语法高亮按栏内绝对行号寻址
                let fg_spans = match (pane, cell) {
                    (Pane::Local, Cell::Line { no, .. }) => pane_fg(highlight.ours.as_ref(), *no),
                    (Pane::Result, Cell::Line { no, .. }) => {
                        pane_fg(highlight.result.as_ref(), *no)
                    }
                    (Pane::Remote, Cell::Line { no, .. }) => {
                        pane_fg(highlight.theirs.as_ref(), *no)
                    }
                    _ => &[],
                };
                match row.fold {
                    Some(n) => fold_line(n, pane, inner.width, theme),
                    None => cell_line(row, cell, pane, &budget, theme, emphasis, fg_spans),
                }
            })
            .collect();
        frame.render_widget(block, col);
        frame.render_widget(Paragraph::new(lines), inner);
    }
    // 三栏同步滚动,滚动条只画一条,置于窗口最右缘,避免栏间边框被滑块「点亮」
    draw_scrollbar(frame, cols[2], max_scroll, scroll, theme);
}

/// 在栏右边框上覆画滚动条(避开圆角);内容未溢出时不显示。
fn draw_scrollbar(frame: &mut Frame, col: Rect, max_scroll: usize, scroll: usize, theme: &Theme) {
    if max_scroll == 0 || col.height <= 2 || col.width < 2 {
        return;
    }
    let track = Rect {
        x: col.x + col.width - 1,
        y: col.y + 1,
        width: 1,
        height: col.height - 2,
    };
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(Style::new().fg(theme.border))
        .thumb_symbol("┃")
        .thumb_style(Style::new().fg(theme.scrollbar_thumb));
    let mut state = ScrollbarState::new(max_scroll).position(scroll);
    frame.render_stateful_widget(scrollbar, track, &mut state);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 三层区间交叠时分段与样式合成正确
    #[test]
    fn compose_spans_merges_overlapping_layers() {
        let fg = [(Color::Red, 0..4)];
        let emphasis = std::slice::from_ref(&(2..6));
        let spans = compose_spans("abcdef", &fg, emphasis, Color::Blue);
        // 分段:[0,2) 仅 fg | [2,4) fg + emph bg | [4,6) 仅 emph bg
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content, "ab");
        assert_eq!(
            (spans[0].style.fg, spans[0].style.bg),
            (Some(Color::Red), None)
        );
        assert_eq!(spans[1].content, "cd");
        assert_eq!(
            (spans[1].style.fg, spans[1].style.bg),
            (Some(Color::Red), Some(Color::Blue))
        );
        assert_eq!(spans[2].content, "ef");
        assert_eq!(
            (spans[2].style.fg, spans[2].style.bg),
            (None, Some(Color::Blue))
        );
    }

    /// 区间端点越界时被安全钳制,不会切出非法切片
    #[test]
    fn compose_spans_clamps_out_of_range() {
        let spans = compose_spans("ab", &[], std::slice::from_ref(&(1..99)), Color::Blue);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].content, "b");
        assert_eq!(spans[1].style.bg, Some(Color::Blue));
    }
}
