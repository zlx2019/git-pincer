//! 三栏正文渲染:圆角边框面板、色带、词级强调、选中指示、行号、折叠横线与滚动条。

use crate::i18n::{tr, tr_f};
use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use unicode_width::UnicodeWidthChar;

use crate::app::{FileMerge, SideState};

use super::highlight::{FileHighlight, PaneSyntax};
use super::rows::{Cell, ChangeType, Row};
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

/// 水平平移的固定步长(显示列)。
const HSCROLL_STEP: usize = 8;

/// 一栏内的空间预算:窄终端时逐级裁剪次要元素(先丢行号,再丢 gutter)。
struct CellBudget {
    /// 是否显示左侧 gutter 列(操作符号 + 空格)
    gutter: bool,
    /// 行号列宽;0 表示不显示行号
    no_width: usize,
    /// 代码内容的可用显示列数(扣除 gutter 与行号列)
    content: usize,
    /// 水平平移列偏移(三栏共用)
    hscroll: usize,
}

/// 依据栏内宽与最大行号计算空间预算(hscroll 由绘制流程结算后注入)。
fn cell_budget(inner: u16, max_no: usize) -> CellBudget {
    let no_width = max_no.to_string().len().max(3);
    let (gutter, no_width) = if inner >= 24 {
        (true, no_width)
    } else if inner >= 14 {
        (true, 0)
    } else {
        (false, 0)
    };
    let prefix = usize::from(gutter) * 2 + if no_width > 0 { no_width + 1 } else { 0 };
    CellBudget {
        gutter,
        no_width,
        content: (inner as usize).saturating_sub(prefix),
        hscroll: 0,
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
        tr_f("ui.fold", &[("n", &n.to_string())])
    } else if inner >= 12 {
        format!("─ ⋯ {n} ⋯ ─")
    } else {
        "─".repeat(inner as usize)
    };
    Line::styled(text, style).alignment(Alignment::Center)
}

/// 一行文本的显示列宽(CJK 等宽字符计 2 列)。
fn display_width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// 水平裁剪结果:可见文本与样式区间的平移参数。
struct HClip {
    /// 可见文本(左边界切半的宽字符已让位为空格)
    text: String,
    /// 原文中的可见字节区间
    span: Range<usize>,
    /// text 头部的补位空格字节数(样式区间平移时计入偏移)
    lead_pad: usize,
    /// 已产出的显示列数(含补位;右侧钉住截断指示时据此补齐)
    cols: usize,
}

/// 从 text 中切出显示列区间 `[skip, skip+take)`:
/// 宽字符跨越左边界时整体让位、以空格补齐视觉列;
/// 右边界放不下的宽字符直接舍弃(不足一列不硬塞)。
fn hclip(text: &str, skip: usize, take: usize) -> HClip {
    let mut start = text.len();
    let mut end = text.len();
    let mut lead_pad = 0;
    let mut col = 0; // 当前字符的起始显示列
    let mut used = 0; // 已产出的可见列数(含补位)
    let mut started = false;
    for (i, c) in text.char_indices() {
        let w = c.width().unwrap_or(0);
        if !started {
            if col + w <= skip {
                col += w;
                continue;
            }
            started = true;
            start = i;
            if col < skip {
                // 宽字符跨越左边界:整体跳过,可见余量以空格补位
                lead_pad = col + w - skip;
                used = lead_pad.min(take);
                start = i + c.len_utf8();
                col += w;
                continue;
            }
        }
        if used + w > take {
            end = i;
            break;
        }
        used += w;
        col += w;
    }
    let mut out = " ".repeat(lead_pad.min(take));
    out.push_str(&text[start.min(end)..end]);
    HClip {
        text: out,
        span: start.min(end)..end,
        lead_pad: lead_pad.min(take),
        cols: used,
    }
}

/// 把原文的样式字节区间平移进裁剪后文本;与可见区间无交集返回 None。
fn shift_range(r: &Range<usize>, clip: &HClip) -> Option<Range<usize>> {
    let s = r.start.max(clip.span.start);
    let e = r.end.min(clip.span.end);
    if s >= e {
        return None;
    }
    Some((s - clip.span.start + clip.lead_pad)..(e - clip.span.start + clip.lead_pad))
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

/// 一行的两个着色层:(词级强调区间, 语法高亮段)。
type LineLayers<'a> = (&'a [Range<usize>], &'a [(Color, Range<usize>)]);

/// 组装一栏中的一行(gutter 符号 + 行号 + 内容 / 占位 / 空白);
/// `current` 为该行是否属于光标所在块(渲染时现算,行列表可跨帧缓存),
/// `(emphasis, fg_spans)` 为词级强调与语法高亮两个着色层。
fn cell_line(
    row: &Row,
    current: bool,
    cell: &Cell,
    pane: Pane,
    budget: &CellBudget,
    theme: &Theme,
    (emphasis, fg_spans): LineLayers<'_>,
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
        && let Some(bg) = theme.band_bg(row.change, current)
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
                tr("ui.pending"),
                Style::new().fg(theme.placeholder_fg),
            ));
            return Line::from(spans).style(style);
        }
    };
    if budget.no_width > 0 {
        let no_style = if current {
            Style::new().fg(theme.fg_bright)
        } else {
            Style::new().fg(theme.fg_dim)
        };
        spans.push(Span::styled(
            format!("{no:>w$} ", w = budget.no_width),
            no_style,
        ));
    }
    // 水平平移与截断:内容窗口为显示列区间 [hscroll, hscroll+content),
    // 左/右边界外仍有内容时,各用窗口边缘一列淡色省略号指示
    let total = display_width(text);
    if budget.hscroll == 0 && total <= budget.content {
        // 快路径:无平移且未溢出,原样渲染
        if emphasis.is_empty() && fg_spans.is_empty() {
            spans.push(Span::raw(text.clone()));
        } else {
            spans.extend(compose_spans(
                text,
                fg_spans,
                emphasis,
                theme.emph_bg(row.change, current),
            ));
        }
        return Line::from(spans).style(style);
    }
    let lead = budget.hscroll > 0;
    let tail = total > budget.hscroll + budget.content;
    let dim = Style::new().fg(theme.fg_dim);
    let skip = budget.hscroll + usize::from(lead);
    let take = budget
        .content
        .saturating_sub(usize::from(lead) + usize::from(tail));
    let clip = hclip(text, skip, take);
    if lead {
        spans.push(Span::styled("…", dim));
    }
    if emphasis.is_empty() && fg_spans.is_empty() {
        spans.push(Span::raw(clip.text.clone()));
    } else {
        let fg_adj: Vec<(Color, Range<usize>)> = fg_spans
            .iter()
            .filter_map(|(c, r)| shift_range(r, &clip).map(|r| (*c, r)))
            .collect();
        let emph_adj: Vec<Range<usize>> = emphasis
            .iter()
            .filter_map(|r| shift_range(r, &clip))
            .collect();
        spans.extend(compose_spans(
            &clip.text,
            &fg_adj,
            &emph_adj,
            theme.emph_bg(row.change, current),
        ));
    }
    if tail {
        // 截断指示钉在窗口最后一列:与内容间的列差(宽字符舍弃残留)以空格补齐
        let gap = take.saturating_sub(clip.cols);
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        spans.push(Span::styled("…", dim));
    }
    Line::from(spans).style(style)
}

/// 取某栏某行的语法高亮段;无语法定义或行号越界时为空。
fn pane_fg(pane: Option<&PaneSyntax>, no: usize) -> &[(Color, Range<usize>)] {
    pane.and_then(|p| p.lines.get(no.wrapping_sub(1)))
        .map_or(&[], Vec::as_slice)
}

/// 三列正文;根据光标调整滚动位置并写回,跨帧保持视口稳定。
///
/// 渲染行由 [`RowCache`](super::rows::RowCache) 提供(纯导航按键零重建),
/// 光标所在块在此处用 `row.chunk` 现比;`(scroll_request, hscroll_request)`
/// 为待结算的手动滚动量(竖直半页 / 水平固定列步长为单位),
/// 竖直滚动消费后视口脱离光标跟随,水平平移三栏联动且不影响跟随。
pub(crate) fn draw_columns(
    frame: &mut Frame,
    area: Rect,
    merge: &mut FileMerge,
    theme: &Theme,
    highlight: &FileHighlight,
    (rows, chunk_starts, max_no): (&[Row], &[usize], usize),
    (scroll_request, hscroll_request): (&mut isize, &mut isize),
) {
    // 上下边框各占一行
    let height = (area.height as usize).saturating_sub(2);
    let max_scroll = rows.len().saturating_sub(height);

    // 结算手动滚动:半页步长按当前视口高度换算,滚动后脱离跟随
    if *scroll_request != 0 {
        let step = (height / 2).max(1) as isize;
        merge.scroll = merge
            .scroll
            .saturating_add_signed(*scroll_request * step)
            .min(max_scroll);
        merge.follow = false;
        *scroll_request = 0;
    }

    // 跟随模式下:光标块在视口内则不动;跳出视口时把块首行定位到约 1/3 处,
    // 让上方留出上下文、下方能看到块的内容
    let target = chunk_starts.get(merge.cursor).copied().unwrap_or(0);
    let mut scroll = merge.scroll.min(max_scroll);
    if merge.follow && height > 0 && (target < scroll || target >= scroll + height) {
        scroll = target.saturating_sub(height / 3).min(max_scroll);
    }
    merge.scroll = scroll;

    let visible = &rows[scroll.min(rows.len())..rows.len().min(scroll + height)];

    let cols = split_columns(area);
    let panes = [
        (Pane::Local, cols[0]),
        (Pane::Result, cols[1]),
        (Pane::Remote, cols[2]),
    ];
    let mut budgets = panes.map(|(_, col)| cell_budget(col.width.saturating_sub(2), max_no));

    // 结算水平平移:上限取三栏中最大的溢出量,行集变化后始终钳回有效范围
    let max_hscroll = max_h_overflow(visible, &budgets);
    merge.hscroll = merge
        .hscroll
        .saturating_add_signed(*hscroll_request * HSCROLL_STEP as isize)
        .min(max_hscroll);
    *hscroll_request = 0;
    for budget in &mut budgets {
        budget.hscroll = merge.hscroll;
    }

    for (idx, (pane, col)) in panes.into_iter().enumerate() {
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
        let budget = &budgets[idx];
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
                // 语法高亮寻址:左右栏按栏内绝对行号,结果栏按 (块, 块内偏移)
                let fg_spans: &[(Color, Range<usize>)] = match (pane, cell) {
                    (Pane::Local, Cell::Line { no, .. }) => pane_fg(highlight.ours.as_ref(), *no),
                    (Pane::Result, Cell::Line { offset, .. }) => highlight
                        .result
                        .as_ref()
                        .map_or(&[], |r| r.spans(row.chunk, *offset)),
                    (Pane::Remote, Cell::Line { no, .. }) => {
                        pane_fg(highlight.theirs.as_ref(), *no)
                    }
                    _ => &[],
                };
                // 光标所在块(折叠行只出现在稳定块上,恒非当前)
                let current = row.chunk == merge.cursor && row.change != ChangeType::None;
                match row.fold {
                    Some(n) => fold_line(n, pane, inner.width, theme),
                    None => cell_line(
                        row,
                        current,
                        cell,
                        pane,
                        budget,
                        theme,
                        (emphasis, fg_spans),
                    ),
                }
            })
            .collect();
        frame.render_widget(block, col);
        frame.render_widget(Paragraph::new(lines), inner);
    }
    // 三栏同步滚动,滚动条只画一条,置于窗口最右缘,避免栏间边框被滑块「点亮」
    draw_scrollbar(frame, cols[2], max_scroll, scroll, theme);
    // 水平平移同理只画一条,置于中栏底边框
    draw_hscrollbar(frame, cols[1], max_hscroll, merge.hscroll, theme);
}

/// 三栏各自可见行宽相对其内容宽的溢出列数,取最大者作为水平平移上限。
fn max_h_overflow(visible: &[Row], budgets: &[CellBudget; 3]) -> usize {
    let mut max = 0;
    for row in visible.iter().filter(|r| r.fold.is_none()) {
        let cells = [
            (&row.ours, &budgets[0]),
            (&row.result, &budgets[1]),
            (&row.theirs, &budgets[2]),
        ];
        for (cell, budget) in cells {
            if let Cell::Line { text, .. } = cell {
                max = max.max(display_width(text).saturating_sub(budget.content));
            }
        }
    }
    max
}

/// 在中栏底边框上覆画水平滚动条(避开圆角);内容未溢出时不显示。
fn draw_hscrollbar(
    frame: &mut Frame,
    col: Rect,
    max_hscroll: usize,
    hscroll: usize,
    theme: &Theme,
) {
    if max_hscroll == 0 || col.width <= 2 || col.height < 2 {
        return;
    }
    let track = Rect {
        x: col.x + 1,
        y: col.y + col.height - 1,
        width: col.width - 2,
        height: 1,
    };
    let scrollbar = Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("─"))
        .track_style(Style::new().fg(theme.border))
        .thumb_symbol("━")
        .thumb_style(Style::new().fg(theme.scrollbar_thumb));
    let mut state = ScrollbarState::new(max_hscroll).position(hscroll);
    frame.render_stateful_widget(scrollbar, track, &mut state);
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

    /// ASCII 行按列窗口切片:区间与产出列数正确
    #[test]
    fn hclip_ascii_window() {
        let clip = hclip("abcdefgh", 2, 3);
        assert_eq!(clip.text, "cde");
        assert_eq!(clip.span, 2..5);
        assert_eq!((clip.lead_pad, clip.cols), (0, 3));
    }

    /// 宽字符跨越左边界:整体让位,可见余列以空格补位
    #[test]
    fn hclip_wide_char_straddles_left_edge() {
        // "你"占 0..2 列,窗口从第 1 列开始 → 让位补 1 空格
        let clip = hclip("你好ab", 1, 4);
        assert_eq!(clip.text, " 好a");
        assert_eq!(clip.span, 3..7);
        assert_eq!((clip.lead_pad, clip.cols), (1, 4));
    }

    /// 宽字符在右边界放不下:整体舍弃,产出列数不足由调用方补齐
    #[test]
    fn hclip_wide_char_dropped_at_right_edge() {
        let clip = hclip("ab你好", 0, 3);
        assert_eq!(clip.text, "ab");
        assert_eq!(clip.cols, 2);
    }

    /// 窗口整体在行尾之后:产出空串,不越界
    #[test]
    fn hclip_window_past_line_end() {
        let clip = hclip("ab", 5, 3);
        assert_eq!(clip.text, "");
        assert_eq!(clip.cols, 0);
    }

    /// 样式区间平移:与可见区间求交后按补位偏移;无交集丢弃
    #[test]
    fn shift_range_translates_and_drops() {
        let clip = hclip("你好ab", 1, 4); // span 3..7, lead_pad 1
        assert_eq!(shift_range(&(0..4), &clip), Some(1..2)); // "你"被裁,"好"保留
        assert_eq!(shift_range(&(3..7), &clip), Some(1..5));
        assert_eq!(shift_range(&(7..8), &clip), None); // 完全在窗口右侧
        assert_eq!(shift_range(&(0..3), &clip), None); // 完全在窗口左侧
    }

    /// 显示宽度:CJK 计 2 列
    #[test]
    fn display_width_counts_wide_chars() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("你a"), 3);
    }
}
