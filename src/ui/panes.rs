//! Three-pane rendering: rounded borders, bands, word emphasis, selection, line numbers, folds, and scrollbars.

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

/// One of the three panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    /// Local (left pane, ours)
    Local,
    /// Merge result (middle pane)
    Result,
    /// Remote (right pane, theirs)
    Remote,
}

/// Fixed horizontal scroll step in display columns.
const HSCROLL_STEP: usize = 8;

/// Pane width budget; narrow terminals drop line numbers first, then the gutter.
struct CellBudget {
    /// Whether to show the left gutter (action symbol + space)
    gutter: bool,
    /// Line-number column width; 0 hides line numbers
    no_width: usize,
    /// Display columns available for code after the gutter and line numbers
    content: usize,
    /// Horizontal column offset shared by all panes
    hscroll: usize,
}

/// Compute the budget from inner width and maximum line number; drawing supplies hscroll later.
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

/// IDEA-style gutter symbols: inward arrows for pending changes,
/// a check mark for applied changes, and a cross for ignored changes.
/// The result pane has no gutter symbols to avoid appearing to recolor its border;
/// brighter bands and line numbers identify the current chunk.
fn gutter_symbol(row: &Row, pane: Pane, theme: &Theme) -> Span<'static> {
    let sym = |state: Option<SideState>, arrow: &'static str| match state {
        // An editor override resolves the chunk, so no pending-take hint is needed.
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

/// Pane title embedded in its border: fixed pane name plus branch label.
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

/// Fold row: plain rules in side panes, a line-count label in the middle pane,
/// simplified as width shrinks. Rules share the border color for consistency.
fn fold_line(n: usize, pane: Pane, inner: u16, theme: &Theme) -> Line<'static> {
    let style = Style::new().fg(theme.border);
    if pane != Pane::Result {
        return Line::styled("─".repeat(inner as usize), style);
    }
    // Simplify the middle label as width shrinks: full text, compact text, then a plain rule.
    let text = if inner >= 30 {
        tr_f("ui.fold", &[("n", &n.to_string())])
    } else if inner >= 12 {
        format!("─ ⋯ {n} ⋯ ─")
    } else {
        "─".repeat(inner as usize)
    };
    Line::styled(text, style).alignment(Alignment::Center)
}

/// Display width of a text line; wide characters such as CJK occupy two columns.
fn display_width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Horizontal clipping result: visible text and offsets for translating style ranges.
struct HClip {
    /// Visible text, replacing wide characters crossing the left edge with spaces
    text: String,
    /// Visible byte range in the original text
    span: Range<usize>,
    /// Leading padding bytes in text, included when translating style ranges
    lead_pad: usize,
    /// Produced display columns including padding, used to position the right truncation marker
    cols: usize,
}

/// Clip text to display columns `[skip, skip+take)`.
/// Replace wide characters crossing the left edge with padding spaces;
/// omit wide characters that do not fit at the right edge.
fn hclip(text: &str, skip: usize, take: usize) -> HClip {
    let mut start = text.len();
    let mut end = text.len();
    let mut lead_pad = 0;
    let mut col = 0; // Starting display column of the current character.
    let mut used = 0; // Produced visible columns, including padding.
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
                // Skip a wide character crossing the left edge and pad its visible remainder.
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

/// Translate an original style range into clipped text; return None if it is not visible.
fn shift_range(r: &Range<usize>, clip: &HClip) -> Option<Range<usize>> {
    let s = r.start.max(clip.span.start);
    let e = r.end.min(clip.span.end);
    if s >= e {
        return None;
    }
    Some((s - clip.span.start + clip.lead_pad)..(e - clip.span.start + clip.lead_pad))
}

/// Compose styles at syntax-foreground and word-emphasis range boundaries.
/// Use syntax foregrounds and brighter emphasis backgrounds over the line's band background.
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

/// Two color layers for a line: (word-emphasis ranges, syntax spans).
type LineLayers<'a> = (&'a [Range<usize>], &'a [(Color, Range<usize>)]);

/// Build a pane row: gutter symbol, line number, and content / placeholder / blank.
/// `current` is computed during rendering so rows can remain cached across frames;
/// `(emphasis, fg_spans)` supplies word-emphasis and syntax color layers.
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
    // Bands cover changed panes and the result, disappearing on resolution; brighten the current chunk.
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
    // The content window spans display columns [hscroll, hscroll+content).
    // A dim ellipsis at either edge indicates additional content outside the window.
    let total = display_width(text);
    if budget.hscroll == 0 && total <= budget.content {
        // Fast path: render unchanged when unscrolled and fully within the available width.
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
        // Pin the marker to the final column, padding any gap left by omitted wide characters.
        let gap = take.saturating_sub(clip.cols);
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        spans.push(Span::styled("…", dim));
    }
    Line::from(spans).style(style)
}

/// Get syntax spans for a pane line; empty without syntax or for an out-of-range line.
fn pane_fg(pane: Option<&PaneSyntax>, no: usize) -> &[(Color, Range<usize>)] {
    pane.and_then(|p| p.lines.get(no.wrapping_sub(1)))
        .map_or(&[], Vec::as_slice)
}

/// Draw the three-pane body, updating scroll positions to keep the viewport stable across frames.
///
/// [`RowCache`](super::rows::RowCache) avoids rebuilding rows during navigation;
/// compare `row.chunk` with the cursor here. Pending scroll requests use vertical
/// half-pages and fixed horizontal column steps. Vertical scrolling disables cursor
/// following; horizontal scrolling moves all panes without affecting following.
pub(crate) fn draw_columns(
    frame: &mut Frame,
    area: Rect,
    merge: &mut FileMerge,
    theme: &Theme,
    highlight: &FileHighlight,
    (rows, chunk_starts, max_no): (&[Row], &[usize], usize),
    (scroll_request, hscroll_request): (&mut isize, &mut isize),
) {
    // Top and bottom borders each occupy one row.
    let height = (area.height as usize).saturating_sub(2);
    let max_scroll = rows.len().saturating_sub(height);

    // Convert half-page requests using viewport height and disable cursor following.
    if *scroll_request != 0 {
        let step = (height / 2).max(1) as isize;
        merge.scroll = merge
            .scroll
            .saturating_add_signed(*scroll_request * step)
            .min(max_scroll);
        merge.follow = false;
        *scroll_request = 0;
    }

    // When following, keep visible chunks in place; otherwise position their first row
    // about one-third down, leaving context above and room for chunk content below.
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

    // Clamp horizontal scrolling to the largest pane overflow, including after row changes.
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
                // Emphasize words only in unresolved side panes; the result shows chosen content.
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
                // Syntax lookup uses absolute side-pane line numbers and (chunk, offset) for results.
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
                // Cursor chunk; fold rows belong only to stable chunks and are never current.
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
    // One scrollbar at the far right serves all panes without highlighting internal borders.
    draw_scrollbar(frame, cols[2], max_scroll, scroll, theme);
    // Likewise, draw one horizontal scrollbar on the middle pane's bottom border.
    draw_hscrollbar(frame, cols[1], max_hscroll, merge.hscroll, theme);
}

/// Maximum visible-line overflow across panes, defining the horizontal scroll limit.
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

/// Overlay a horizontal scrollbar on the middle bottom border, avoiding corners; hide without overflow.
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

/// Overlay a scrollbar on the right border, avoiding corners; hide without overflow.
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

/// Divide the area into three equal horizontal sections.
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

    /// Overlapping style layers are segmented and composed correctly.
    #[test]
    fn compose_spans_merges_overlapping_layers() {
        let fg = [(Color::Red, 0..4)];
        let emphasis = std::slice::from_ref(&(2..6));
        let spans = compose_spans("abcdef", &fg, emphasis, Color::Blue);
        // Segments: [0,2) foreground | [2,4) foreground + emphasis | [4,6) emphasis only.
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

    /// Out-of-range endpoints are safely clamped to avoid invalid slices.
    #[test]
    fn compose_spans_clamps_out_of_range() {
        let spans = compose_spans("ab", &[], std::slice::from_ref(&(1..99)), Color::Blue);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].content, "b");
        assert_eq!(spans[1].style.bg, Some(Color::Blue));
    }

    /// Clipping ASCII lines produces correct ranges and column counts.
    #[test]
    fn hclip_ascii_window() {
        let clip = hclip("abcdefgh", 2, 3);
        assert_eq!(clip.text, "cde");
        assert_eq!(clip.span, 2..5);
        assert_eq!((clip.lead_pad, clip.cols), (0, 3));
    }

    /// Wide characters crossing the left edge are replaced with padding spaces.
    #[test]
    fn hclip_wide_char_straddles_left_edge() {
        // The first CJK character occupies columns 0..2; starting at 1 leaves one padding space.
        let clip = hclip("你好ab", 1, 4);
        assert_eq!(clip.text, " 好a");
        assert_eq!(clip.span, 3..7);
        assert_eq!((clip.lead_pad, clip.cols), (1, 4));
    }

    /// Omit wide characters that do not fit at the right edge; the caller pads remaining columns.
    #[test]
    fn hclip_wide_char_dropped_at_right_edge() {
        let clip = hclip("ab你好", 0, 3);
        assert_eq!(clip.text, "ab");
        assert_eq!(clip.cols, 2);
    }

    /// A window past the line end produces an empty string without going out of bounds.
    #[test]
    fn hclip_window_past_line_end() {
        let clip = hclip("ab", 5, 3);
        assert_eq!(clip.text, "");
        assert_eq!(clip.cols, 0);
    }

    /// Intersect style ranges with visible bytes, translate for padding, and discard invisible ranges.
    #[test]
    fn shift_range_translates_and_drops() {
        let clip = hclip("你好ab", 1, 4); // span 3..7, lead_pad 1
        assert_eq!(shift_range(&(0..4), &clip), Some(1..2)); // First CJK character clipped, second retained.
        assert_eq!(shift_range(&(3..7), &clip), Some(1..5));
        assert_eq!(shift_range(&(7..8), &clip), None); // Entirely right of the window.
        assert_eq!(shift_range(&(0..3), &clip), None); // Entirely left of the window.
    }

    /// CJK characters occupy two display columns.
    #[test]
    fn display_width_counts_wide_chars() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("你a"), 3);
    }
}
