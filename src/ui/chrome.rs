//! Overall UI rendering: status, hints, messages, help overlay, and whole-file picks.

use crate::i18n::{tr, tr_f};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use crate::app::{FileEntry, PickKind, Session, Side};

use super::UiState;
use super::keymap::{Action, key_hint};
use super::panes::draw_columns;
use super::theme::Theme;

/// Draw the entire UI; public for smoke testing with TestBackend.
///
/// The session is mutable to persist scroll positions across frames.
/// UI state is mutable to build highlighting caches on demand.
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
            let rows = ui.rows.get(file_idx, merge, ui.revision, folded);
            draw_columns(
                frame,
                body,
                merge,
                &ui.theme,
                highlight,
                rows,
                (&mut ui.scroll_request, &mut ui.hscroll_request),
            );
        }
        FileEntry::Pick {
            path,
            kind,
            ours,
            theirs,
            choice,
        } => draw_pick(
            frame,
            body,
            path,
            pick_notice(*kind, ours.is_none() || theirs.is_none()),
            [
                side_summary(*kind, ours.as_deref()),
                side_summary(*kind, theirs.as_deref()),
            ],
            *choice,
            &ui.theme,
        ),
    }
    // Whole-file picks do not consume scroll requests; clear them before moving to text files.
    ui.scroll_request = 0;
    ui.hscroll_request = 0;
    draw_hints(frame, hints, &ui.theme);
    frame.render_widget(
        Paragraph::new(ui.message.as_str()).style(Style::new().fg(ui.theme.amber)),
        message,
    );
    if ui.show_help {
        draw_help(frame, &ui.theme);
    }
}

/// Top status bar: operation badge, progress gauge, file path, and conflict count.
fn draw_header(frame: &mut Frame, area: Rect, session: &Session, theme: &Theme) {
    let entry = session.current_file();
    let pending = match entry {
        FileEntry::Text(m) => m.pending_conflicts(),
        FileEntry::Pick { choice, .. } => usize::from(choice.is_none()),
    };
    let done = session.written.iter().filter(|&&w| w).count();
    let total = session.files.len();

    // Progress gauge: written files / total files.
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
            format!(
                " {} ",
                tr_f(
                    "ui.files_badge",
                    &[
                        ("i", &(session.current + 1).to_string()),
                        ("n", &total.to_string()),
                    ],
                )
            ),
            Style::new().fg(theme.fg_bright),
        ),
        sep.clone(),
        Span::raw(format!(" {} ", entry.path())),
        sep,
    ];
    if pending > 0 {
        spans.push(Span::styled(
            format!(
                " {}",
                tr_f("ui.conflicts_pending", &[("n", &pending.to_string())])
            ),
            Style::new().fg(theme.red).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            format!(" {}", tr("ui.file_ready")),
            Style::new().fg(theme.green),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Explain why the object requires a whole-file pick rather than chunk merging.
/// A regular file missing on one side is described as a modify/delete conflict.
fn pick_notice(kind: PickKind, has_deleted: bool) -> &'static str {
    match kind {
        PickKind::File if has_deleted => tr("ui.deleted_notice"),
        PickKind::File => tr("ui.binary_notice"),
        PickKind::Symlink => tr("ui.symlink_notice"),
        PickKind::Submodule => tr("ui.submodule_notice"),
    }
}

/// Summarize one side: deleted, link target, submodule commit, or byte count.
fn side_summary(kind: PickKind, content: Option<&[u8]>) -> String {
    let Some(content) = content else {
        return tr("ui.side_deleted").to_owned();
    };
    match kind {
        PickKind::File => tr_f("ui.bytes", &[("n", &content.len().to_string())]),
        PickKind::Symlink => tr_f(
            "ui.link_target",
            &[("target", &String::from_utf8_lossy(content))],
        ),
        PickKind::Submodule => {
            // Use the first seven commit ID characters, matching git log --oneline.
            let id = String::from_utf8_lossy(content);
            tr_f("ui.commit_id", &[("id", id.get(..7).unwrap_or(&id))])
        }
    }
}

/// Fallback for conflicts that cannot be merged by chunk: choose a LOCAL or REMOTE card.
fn draw_pick(
    frame: &mut Frame,
    area: Rect,
    path: &str,
    notice: &str,
    [ours, theirs]: [String; 2],
    choice: Option<Side>,
    theme: &Theme,
) {
    let panel = centered_rect(area, 64, 11);
    let [title, notice_area, _, cards, _, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(panel);

    frame.render_widget(
        Paragraph::new(
            Line::styled(path.to_owned(), Style::new().add_modifier(Modifier::BOLD)).centered(),
        ),
        title,
    );
    // Wrap explanations that exceed pane width, such as English text, to preserve sentence endings.
    frame.render_widget(
        Paragraph::new(Line::styled(notice, Style::new().fg(theme.amber)).centered())
            .wrap(Wrap { trim: true }),
        notice_area,
    );

    let [left, _, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Fill(1),
    ])
    .areas(cards);
    // Derive key labels from effective bindings so overrides match actual dispatch.
    let (left_key, right_key) = (key_hint(Action::TakeLocal), key_hint(Action::TakeRemote));
    pick_card(
        frame,
        left,
        " LOCAL ",
        &left_key,
        &ours,
        choice == Some(Side::Ours),
        theme,
    );
    pick_card(
        frame,
        right,
        " REMOTE ",
        &right_key,
        &theirs,
        choice == Some(Side::Theirs),
        theme,
    );

    let keys = tr_f(
        "ui.binary_keys",
        &[
            ("left", &left_key),
            ("right", &right_key),
            ("undo", &key_hint(Action::UndoChunk)),
            ("write", &key_hint(Action::WriteFile)),
        ],
    );
    frame.render_widget(
        Paragraph::new(Line::styled(keys, Style::new().fg(theme.fg_dim))).centered(),
        hint,
    );
}

/// Candidate card with a content summary; selection adds a green border and title check mark.
fn pick_card(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    key: &str,
    summary: &str,
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
            Line::styled(summary.to_owned(), Style::new().fg(theme.fg_bright)).centered(),
            Line::styled(
                tr_f("ui.press_pick", &[("key", key)]),
                Style::new().fg(theme.fg_dim),
            )
            .centered(),
        ]),
        inner,
    );
}

/// Keycap-style Span with a subtly raised background.
fn keycap(key: &str, theme: &Theme) -> Span<'static> {
    Span::styled(
        format!(" {key} "),
        Style::new().fg(theme.keycap_fg).bg(theme.keycap_bg),
    )
}

/// Bottom key hints (keycaps and descriptions) from the shared [`keymap`](super::keymap) table.
fn draw_hints(frame: &mut Frame, area: Rect, theme: &Theme) {
    let mut spans = vec![Span::raw(" ")];
    for (key, desc) in super::keymap::hint_entries() {
        spans.push(keycap(key, theme));
        spans.push(Span::styled(
            format!(" {}  ", tr(desc)),
            Style::new().fg(theme.hint_fg),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Rounded help overlay with two key columns from the shared [`keymap`](super::keymap) table.
fn draw_help(frame: &mut Frame, theme: &Theme) {
    let (left_items, right_items) = super::keymap::help_columns();
    let column = |entries: &[(&'static str, &'static str)]| -> Vec<Line<'static>> {
        entries
            .iter()
            .map(|(key, desc)| {
                Line::from(vec![
                    Span::styled(
                        format!(" {key:<7}"),
                        Style::new().fg(theme.blue).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(tr(desc).to_owned()),
                ])
            })
            .collect()
    };

    // Fit height to the longer column, plus one row each for top and bottom borders.
    let overlay_h = left_items.len().max(right_items.len()) as u16 + 2;
    let area = centered_rect(frame.area(), 78, overlay_h);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .title(Span::styled(
            format!(" {} ", tr("ui.help_title")),
            Style::new()
                .fg(theme.fg_bright)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                format!(" {} ", tr("common.close_any_key")),
                Style::new().fg(theme.fg_dim),
            ))
            .centered(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
    frame.render_widget(Paragraph::new(column(&left_items)), left);
    frame.render_widget(Paragraph::new(column(&right_items)), right);
}

/// Centered overlay rectangle constrained to the screen.
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

    /// Render one TestBackend frame without panicking and check essential content.
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
        // Status-bar operation badge and progress gauge.
        assert!(content.contains("MERGE"));
        assert!(content.contains("░"));
        // Rounded panes with titles embedded in their borders.
        assert!(content.contains("╭"));
        assert!(content.contains("LOCAL"));
        assert!(content.contains("RESULT"));
        assert!(content.contains("REMOTE"));
        // Unresolved conflict: result placeholder and take arrows in both side gutters.
        assert!(content.contains("pending"));
        assert!(content.contains("»"));
        assert!(content.contains("«"));
    }

    /// Narrow terminals (60 columns) render without panicking as width budgets shrink.
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

    /// Regression: conflict navigation keeps the selected chunk visible and above the bottom edge.
    #[test]
    fn scroll_keeps_selected_chunk_in_view() {
        // Three conflicts separated by 40 stable lines; unfold to exceed the viewport height.
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
        // Body height = 24 - one row each for status/hints/messages - two border rows.
        let body_height = 19usize;

        // Jump to the last conflict, then back to the middle one.
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
        assert!(
            target >= m.scroll,
            "selected chunk scrolled above the viewport"
        );
        assert!(
            target < m.scroll + body_height,
            "selected chunk scrolled below the viewport"
        );
        // Leave at least two visible content rows below the chunk's first row.
        assert!(
            target + 2 < m.scroll + body_height,
            "selected chunk is at the viewport bottom (target={target}, scroll={})",
            m.scroll
        );
    }

    /// Manual scrolling disables following without snapping back; cursor actions restore it.
    #[test]
    fn manual_scroll_detaches_and_actions_reattach() {
        let stable: String = (1..=200).map(|i| format!("s{i}\n")).collect();
        let base = format!("a\n{stable}");
        let ours = format!("A\n{stable}");
        let theirs = format!("X\n{stable}");
        let merge = FileMerge::from_three_way("demo.txt".to_owned(), &base, &ours, &theirs);
        let mut session = Session::new(vec![FileEntry::Text(merge)], "merge".to_owned());
        session.folded = false;
        let mut ui = UiState::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let scroll_of = |s: &Session| match s.current_file() {
            FileEntry::Text(m) => (m.scroll, m.follow),
            FileEntry::Pick { .. } => unreachable!(),
        };

        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        assert_eq!(scroll_of(&session), (0, true));

        // Two half-page scrolls consume the request, advance the viewport, and disable following.
        ui.scroll_request = 2;
        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        let (scrolled, follow) = scroll_of(&session);
        assert!(scrolled > 0);
        assert!(!follow);
        assert_eq!(ui.scroll_request, 0);

        // The cursor chunk is outside the viewport; redraw must not snap back.
        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        assert_eq!(scroll_of(&session), (scrolled, false));

        // Cursor actions such as conflict navigation restore following and snap back to the chunk.
        super::super::handle_file_key(
            &mut session,
            super::super::keymap::Action::NextConflict,
            &mut ui,
        );
        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        let (snapped, follow) = scroll_of(&session);
        assert!(follow);
        assert!(
            snapped < scrolled,
            "viewport must snap back to the conflict near the top"
        );
    }

    /// Horizontal scroll uses fixed steps, clamps to overflow, and preserves cursor following.
    #[test]
    fn hscroll_settles_with_step_and_clamp() {
        let long = "x".repeat(200);
        let base = format!("a\n{long}\n");
        let ours = format!("A\n{long}\n");
        let theirs = format!("X\n{long}\n");
        let merge = FileMerge::from_three_way("demo.txt".to_owned(), &base, &ours, &theirs);
        let mut session = Session::new(vec![FileEntry::Text(merge)], "merge".to_owned());
        session.folded = false;
        let mut ui = UiState::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let hscroll_of = |s: &Session| match s.current_file() {
            FileEntry::Text(m) => (m.hscroll, m.follow),
            FileEntry::Pick { .. } => unreachable!(),
        };

        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        assert_eq!(hscroll_of(&session), (0, true));
        // An overflowing line shows a right-edge truncation marker even before scrolling.
        assert!(format!("{:?}", terminal.backend().buffer()).contains('…'));

        // One rightward step consumes the request, moves eight columns, and preserves following.
        ui.hscroll_request = 1;
        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        assert_eq!(hscroll_of(&session), (8, true));
        assert_eq!(ui.hscroll_request, 0);

        // Excessive rightward scrolling clamps to maximum overflow, avoiding an empty viewport.
        ui.hscroll_request = 100;
        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        let (clamped, _) = hscroll_of(&session);
        assert!(clamped >= 8, "must advance to the right");
        assert!(
            clamped < 200,
            "scroll limit must be smaller than the line width"
        );

        // Excessive leftward scrolling clamps to zero.
        ui.hscroll_request = -1000;
        terminal.draw(|f| draw(f, &mut session, &mut ui)).unwrap();
        assert_eq!(hscroll_of(&session).0, 0);
    }

    /// Whole-file pick entry for a binary file.
    fn binary_pick() -> FileEntry {
        FileEntry::Pick {
            path: "logo.png".to_owned(),
            kind: PickKind::File,
            ours: Some(vec![0, 1]),
            theirs: Some(vec![2, 3]),
            choice: None,
        }
    }

    /// Render one entry with TestBackend and return the frame's buffer text.
    fn render_entry(entry: FileEntry) -> String {
        let mut session = Session::new(vec![entry], "merge".to_owned());
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut session, &mut UiState::default()))
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    /// Binary entries render without panicking.
    #[test]
    fn draw_binary_entry_smoke() {
        let content = render_entry(binary_pick());
        assert!(content.contains("logo.png"));
        assert!(content.contains("2 bytes"));
        // Key instructions use effective bindings, which are defaults without overrides.
        assert!(content.contains("press h to choose"));
        assert!(content.contains("h / l choose · u undo · then w to write"));
    }

    /// Wrap explanations wider than the pane (71 English columns > 64) without truncating endings.
    #[test]
    fn binary_notice_wraps_instead_of_clipping() {
        let content = render_entry(binary_pick());
        assert!(content.contains("choose one side as"));
        assert!(
            content.contains("a whole"),
            "explanation sentence ending was truncated"
        );
    }

    /// Symlink cards display targets and correctly label deleted sides.
    #[test]
    fn draw_symlink_entry_shows_targets() {
        let content = render_entry(FileEntry::Pick {
            path: "link".to_owned(),
            kind: PickKind::Symlink,
            ours: Some(b"b.txt".to_vec()),
            theirs: None,
            choice: None,
        });
        assert!(content.contains("Symlink"));
        assert!(content.contains("→ b.txt"));
        assert!(content.contains("(deleted)"));
    }

    /// Modify/delete conflicts have the appropriate explanation and a deleted-side label.
    #[test]
    fn draw_modify_delete_entry_explains_deletion() {
        let content = render_entry(FileEntry::Pick {
            path: "note.txt".to_owned(),
            kind: PickKind::File,
            ours: Some(b"a\nB\nc\n".to_vec()),
            theirs: None,
            choice: None,
        });
        assert!(content.contains("deleted it"));
        assert!(!content.contains("Binary"));
        assert!(content.contains("6 bytes") && content.contains("(deleted)"));
    }

    /// Submodule cards display each side's recorded short commit ID.
    #[test]
    fn draw_submodule_entry_shows_commits() {
        let content = render_entry(FileEntry::Pick {
            path: "vendor/lib".to_owned(),
            kind: PickKind::Submodule,
            ours: Some(b"2222222222222222222222222222222222222222".to_vec()),
            theirs: Some(b"3333333333333333333333333333333333333333".to_vec()),
            choice: None,
        });
        assert!(content.contains("Submodule"));
        assert!(content.contains("commit 2222222"));
        assert!(content.contains("commit 3333333"));
        assert!(
            !content.contains("22222222"),
            "must display a seven-character short ID"
        );
    }
}
