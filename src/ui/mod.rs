//! ratatui three-pane rendering and the key-event loop.
//!
//! Layout: status bar / three bordered panes (local | result | remote)
//! / key hints / message line. Change chunks are tinted as bands on the panes
//! they touch, colored by change type like IDEA (blue = modified, green =
//! added, gray = deleted, red = conflict); the band disappears once a chunk
//! is resolved, the current chunk is highlighted, and `?` shows the full key
//! reference.
//!
//! Modules:
//! - [`theme`][]: centralized color definitions
//! - [`keymap`][]: shared key bindings for dispatch, hints, and help
//! - [`rows`][]: render-row structures and construction, including folding and placeholders
//! - [`highlight`][]: word emphasis and syntax highlighting computation and caching
//! - [`panes`][]: three-pane body rendering
//! - [`chrome`][]: status bar, hints, help overlay, and binary view
//! - This module: event loop and key dispatch

mod chrome;
mod highlight;
pub(crate) mod keymap;
mod menu;
mod panes;
mod rows;
mod theme;

use crate::i18n::{tr, tr_f};
use std::io::IsTerminal;

use anyhow::{Context, Result};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::app::{FileEntry, Session, Side};
use keymap::Action;

pub use chrome::draw;
pub(crate) use menu::{MenuItem, MenuSession};
pub(crate) use theme::{detect_light, init_overrides as init_theme_overrides};

/// How a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// All files resolved and written
    Completed,
    /// User quit early; the operation remains available to resume
    Quit,
}

/// Transient UI state, including messages and overlay visibility.
#[derive(Debug, Default)]
pub struct UiState {
    /// Bottom message bar content
    pub message: String,
    /// Whether the help overlay is visible
    pub show_help: bool,
    /// Whether another q press will confirm quitting
    pub pending_quit: bool,
    /// UI theme
    pub(crate) theme: theme::Theme,
    /// Word emphasis and syntax highlighting cache
    pub(crate) cache: highlight::HighlightCache,
    /// Render-row cache, reused during pure navigation
    pub(crate) rows: rows::RowCache,
    /// Revision incremented when merge content changes, invalidating result syntax
    /// highlighting and render-row caches
    pub(crate) revision: u64,
    /// Pending manual scroll in half-pages, positive downward; consumed during drawing
    pub(crate) scroll_request: isize,
    /// Pending horizontal scroll in fixed column steps, positive rightward; consumed during drawing
    pub(crate) hscroll_request: isize,
}

/// Run an interactive session until completion or quitting.
///
/// `write_file` persists resolved files and stages them in Git mode; the caller
/// determines how whole-file picks are persisted. `light` selects the light theme
/// for terminals with light backgrounds.
pub fn run_session(
    session: &mut Session,
    write_file: &mut dyn FnMut(&FileEntry) -> Result<()>,
    light: bool,
) -> Result<Outcome> {
    // ratatui::init panics without a TTY; report a readable error first,
    // for example when accidentally invoked in a pipeline or CI.
    if !std::io::stdout().is_terminal() {
        anyhow::bail!("{}", tr("common.need_tty_resolve"));
    }
    run_session_in(ratatui::init(), session, write_file, light)
}

/// Run a session on a transferred terminal, keeping the menu-to-resolver transition
/// in the alternate screen to avoid flicker. Restore the terminal when finished.
///
/// Do not clear first: ratatui's buffer diff replaces the previous page in the first
/// frame without an intermediate blank frame.
pub(crate) fn run_session_in(
    mut terminal: DefaultTerminal,
    session: &mut Session,
    write_file: &mut dyn FnMut(&FileEntry) -> Result<()>,
    light: bool,
) -> Result<Outcome> {
    let result = event_loop(&mut terminal, session, write_file, light);
    ratatui::restore();
    result
}

/// Event loop: draw, read a key, update state.
fn event_loop(
    terminal: &mut DefaultTerminal,
    session: &mut Session,
    write_file: &mut dyn FnMut(&FileEntry) -> Result<()>,
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

        // Any key closes the help overlay.
        if ui.show_help {
            ui.show_help = false;
            continue;
        }
        // Treat Ctrl+C as quit: raw mode does not generate SIGINT, so ignoring it
        // would make the application appear unresponsive.
        let action =
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                Some(Action::Quit)
            } else {
                keymap::action_for(key.code, key.modifiers)
            };
        // Any other key, including an unbound key, cancels quit confirmation.
        if action != Some(Action::Quit) {
            ui.pending_quit = false;
        }
        ui.message.clear();
        let Some(action) = action else {
            continue;
        };

        match action {
            Action::Quit => {
                if session.all_written() || ui.pending_quit {
                    return Ok(Outcome::Quit);
                }
                ui.pending_quit = true;
                ui.message = tr("ui.quit_confirm").to_owned();
            }
            Action::Help => ui.show_help = true,
            Action::NextFile => session.next_file(),
            Action::ToggleFold => session.folded = !session.folded,
            // Record scroll requests; drawing computes and clamps rows using viewport height.
            Action::ScrollDown => ui.scroll_request += 1,
            Action::ScrollUp => ui.scroll_request -= 1,
            Action::ScrollRight => ui.hscroll_request += 1,
            Action::ScrollLeft => ui.hscroll_request -= 1,
            Action::WriteFile => {
                if write_current(session, write_file, &mut ui)? {
                    // Writing automatically applies non-conflicts and may change result content.
                    ui.revision += 1;
                    if session.all_written() {
                        return Ok(Outcome::Completed);
                    }
                }
            }
            Action::EditChunk => {
                if let FileEntry::Text(merge) = session.current_file_mut() {
                    let initial = merge.current_content(merge.cursor);
                    if let Some(lines) = edit_lines(terminal, &initial)? {
                        merge.set_override(lines);
                        ui.revision += 1;
                        ui.message = tr("ui.edited").to_owned();
                    } else {
                        ui.message = tr("ui.edit_cancelled").to_owned();
                    }
                }
            }
            other => {
                if handle_file_key(session, other, &mut ui) {
                    ui.revision += 1;
                }
            }
        }
    }
}

/// Handle current-file actions; return whether content changed to invalidate result highlighting.
fn handle_file_key(session: &mut Session, action: Action, ui: &mut UiState) -> bool {
    match session.current_file_mut() {
        FileEntry::Text(merge) => {
            // Chunk actions restore cursor following; pure scrolling does not reach here.
            merge.follow = true;
            match action {
                Action::TakeLocal => {
                    merge.apply(Side::Ours);
                    true
                }
                Action::TakeRemote => {
                    merge.apply(Side::Theirs);
                    true
                }
                // Ignore all pending sides of the current chunk, preserving applied content.
                Action::IgnoreChunk => {
                    merge.ignore(Side::Ours);
                    merge.ignore(Side::Theirs);
                    true
                }
                Action::UndoChunk => {
                    merge.undo();
                    true
                }
                Action::UndoFile => {
                    merge.undo_all();
                    ui.message = tr("ui.undone_all").to_owned();
                    true
                }
                Action::ApplyNonConflict => {
                    merge.apply_all_nonconflict();
                    ui.message = tr("ui.applied_all").to_owned();
                    true
                }
                Action::NextChange => {
                    merge.next_change();
                    false
                }
                Action::PrevChange => {
                    merge.prev_change();
                    false
                }
                Action::NextConflict => {
                    merge.next_conflict();
                    false
                }
                Action::PrevConflict => {
                    merge.prev_conflict();
                    false
                }
                // Copy keys avoid terminal selections spanning all three panes:
                // chunk result / file result / local chunk / remote chunk.
                Action::CopyChunk => {
                    let lines = merge.current_content(merge.cursor);
                    ui.message = copy_feedback(&lines, tr("ui.copy_chunk"));
                    false
                }
                Action::CopyFile => {
                    ui.message = match copy_to_clipboard(&merge.resolved_content()) {
                        Ok(()) => tr("ui.copied_file").to_owned(),
                        Err(e) => tr_f("ui.copy_failed", &[("e", &e.to_string())]),
                    };
                    false
                }
                Action::CopyLocal => {
                    let lines = merge.chunks[merge.cursor].ours_lines().to_vec();
                    ui.message = copy_feedback(&lines, tr("ui.copy_local"));
                    false
                }
                Action::CopyRemote => {
                    let lines = merge.chunks[merge.cursor].theirs_lines().to_vec();
                    ui.message = copy_feedback(&lines, tr("ui.copy_remote"));
                    false
                }
                _ => false,
            }
        }
        FileEntry::Pick { choice, .. } => {
            match action {
                Action::TakeLocal => *choice = Some(Side::Ours),
                Action::TakeRemote => *choice = Some(Side::Theirs),
                Action::UndoChunk | Action::UndoFile => *choice = None,
                _ => {}
            }
            false
        }
    }
}

/// Copy lines to the clipboard and generate message-bar feedback.
fn copy_feedback(lines: &[String], what: &str) -> String {
    match copy_to_clipboard(&lines.join("\n")) {
        Ok(()) => tr_f(
            "ui.copied",
            &[("what", what), ("n", &lines.len().to_string())],
        ),
        Err(e) => tr_f("ui.copy_failed", &[("e", &e.to_string())]),
    }
}

/// Copy text using pbcopy (macOS), xclip (X11), or wl-copy (Wayland), in order.
/// Fall back to OSC 52 for terminal-managed copying on Windows, SSH, and minimal
/// environments; unsupported terminals silently ignore the sequence.
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
    osc52_copy(text)
}

/// Request terminal-managed clipboard copying through OSC 52.
fn osc52_copy(text: &str) -> Result<()> {
    use std::io::Write as _;
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()?;
    Ok(())
}

/// Standard base64 encoding for the sole OSC 52 use case, avoiding an extra dependency.
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        let sextets = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, sextet) in sextets.iter().enumerate() {
            if i <= chunk.len() {
                out.push(TABLE[*sextet as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Write the current file; return true on success.
///
/// All conflicts must be resolved. Pending non-conflicts are **automatically applied**
/// before writing, matching Git's automatic merge; reject changes explicitly with x.
/// Writing base instead would silently discard changes Git already merged.
fn write_current(
    session: &mut Session,
    write_file: &mut dyn FnMut(&FileEntry) -> Result<()>,
    ui: &mut UiState,
) -> Result<bool> {
    if !session.current_file().ready_to_write() {
        ui.message = tr("ui.unresolved").to_owned();
        return Ok(false);
    }
    let mut auto_applied = 0;
    if let FileEntry::Text(merge) = session.current_file_mut() {
        auto_applied = merge.pending_changes();
        merge.apply_all_nonconflict();
    }
    let entry = session.current_file();
    let path = entry.path().to_owned();
    write_file(entry)?;
    session.mark_written();
    ui.message = if auto_applied > 0 {
        tr_f(
            "ui.written_auto",
            &[("path", &path), ("n", &auto_applied.to_string())],
        )
    } else {
        tr_f("ui.written", &[("path", &path)])
    };
    Ok(true)
}

/// Configured editor (`[ui] editor`), initialized once per process.
static CONFIG_EDITOR: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Apply the editor setting before entering the TUI; only the first call takes effect.
pub(crate) fn init_editor(editor: Option<String>) {
    let _ = CONFIG_EDITOR.set(editor);
}

/// Edit content in an external editor; None means cancellation (nonzero editor exit).
fn edit_lines(terminal: &mut DefaultTerminal, initial: &[String]) -> Result<Option<Vec<String>>> {
    let editor = resolve_editor();
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi").to_owned();
    let args: Vec<&str> = parts.collect();

    let path = std::env::temp_dir().join(format!("git-pincer-edit-{}.txt", std::process::id()));
    std::fs::write(&path, initial.join("\n"))?;

    // Yield the terminal to the editor, then rebuild the TUI.
    ratatui::restore();
    let status = std::process::Command::new(&program)
        .args(&args)
        .arg(&path)
        .status();
    *terminal = ratatui::init();
    terminal.clear()?;

    let status = status.with_context(|| tr_f("ui.editor_failed", &[("program", &program)]))?;
    if !status.success() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);
    // Strip the trailing newline editors usually add to avoid an extra empty line.
    let text = text.strip_suffix('\n').unwrap_or(&text);
    Ok(Some(if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').map(str::to_owned).collect()
    }))
}

/// Editor precedence: configured `[ui].editor` > `$VISUAL` > `$EDITOR` > platform default.
/// Empty values count as unset. Unix checks PATH for vim, then vi, falling back to vi
/// even if absent so startup reports a readable error. Windows defaults to notepad.
fn resolve_editor() -> String {
    let config = CONFIG_EDITOR.get().and_then(|e| e.as_deref());
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    pick_editor(config, visual.as_deref(), editor.as_deref(), on_path)
}

/// Pure, testable editor-selection logic.
fn pick_editor(
    config: Option<&str>,
    visual: Option<&str>,
    editor: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> String {
    let non_empty = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    if let Some(chosen) = non_empty(config)
        .or_else(|| non_empty(visual))
        .or_else(|| non_empty(editor))
    {
        return chosen;
    }
    if cfg!(windows) {
        return "notepad".to_owned();
    }
    for candidate in ["vim", "vi"] {
        if exists(candidate) {
            return candidate.to_owned();
        }
    }
    "vi".to_owned()
}

/// Check whether an executable exists on PATH.
fn on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::FileMerge;

    /// Editor precedence: configuration > VISUAL > EDITOR > vim > vi; empty values are unset.
    #[test]
    #[cfg(not(windows))]
    fn editor_priority_chain() {
        let both = |_: &str| true;
        let none = |_: &str| false;
        let only_vi = |p: &str| p == "vi";

        // Configuration takes precedence; whitespace-only settings are unset.
        assert_eq!(
            pick_editor(Some("code --wait"), Some("nvim"), Some("nano"), both),
            "code --wait"
        );
        assert_eq!(
            pick_editor(Some("  "), Some("nvim"), Some("nano"), both),
            "nvim"
        );
        // VISUAL takes precedence over EDITOR.
        assert_eq!(pick_editor(None, Some("nvim"), Some("nano"), both), "nvim");
        assert_eq!(pick_editor(None, None, Some("nano"), both), "nano");
        // With no settings, prefer vim on PATH, then vi; use vi even if neither exists.
        assert_eq!(pick_editor(None, None, None, both), "vim");
        assert_eq!(pick_editor(None, None, None, only_vi), "vi");
        assert_eq!(pick_editor(None, None, None, none), "vi");
    }

    /// Base64 encoding matches standard test vectors for OSC 52 payloads.
    #[test]
    fn base64_matches_known_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("hello", "aGVsbG8="),
            ("多字节✓", "5aSa5a2X6IqC4pyT"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected, "input: {input:?}");
        }
    }

    /// Regression: writing applies pending non-conflicts instead of reverting to base,
    /// which would silently discard changes Git already merged.
    #[test]
    fn write_auto_applies_pending_nonconflict_changes() {
        let merge = FileMerge::from_three_way(
            "demo.txt".to_owned(),
            "a\nb\nc\nd\n",
            "a\nX\nc\nd\n",
            "a\nY\nc\nD\n",
        );
        let mut session = Session::new(vec![FileEntry::Text(merge)], "merge".to_owned());
        // Resolve only the conflict (take local, ignore remote), leaving theirs' d -> D pending.
        let FileEntry::Text(m) = session.current_file_mut() else {
            unreachable!()
        };
        m.apply(Side::Ours);
        m.ignore(Side::Theirs);

        let mut written: Vec<u8> = Vec::new();
        let mut ui = UiState::default();
        let ok = write_current(
            &mut session,
            &mut |entry| {
                written = entry.resolved_bytes();
                Ok(())
            },
            &mut ui,
        )
        .unwrap();
        assert!(ok);
        assert_eq!(String::from_utf8(written).unwrap(), "a\nX\nc\nD\n");
        assert!(ui.message.contains("auto-applied"));
    }
}
