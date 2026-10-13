//! Central key-binding table: dispatch, the hint bar and the help overlay
//! all derive from one effective table, so a key can never drift out of sync
//! between behavior and documentation.
//!
//! The effective table = built-in defaults + `[keys]` overrides from the user
//! config, frozen once per process via [`init`]. Key labels shown in the UI
//! are derived from the actual key codes, so overridden keys are displayed
//! correctly everywhere for free.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use crate::i18n::tr_f;

/// Bindable actions in a conflict-resolution session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    /// Take the local side
    TakeLocal,
    /// Take the remote side
    TakeRemote,
    /// Ignore pending sides of the current chunk
    IgnoreChunk,
    /// Undo the current chunk's decisions
    UndoChunk,
    /// Undo all decisions in the current file
    UndoFile,
    /// Edit the current chunk in $EDITOR
    EditChunk,
    /// Apply all non-conflicting changes
    ApplyNonConflict,
    /// Move to the next changed chunk
    NextChange,
    /// Move to the previous changed chunk
    PrevChange,
    /// Jump to the next unresolved conflict
    NextConflict,
    /// Jump to the previous unresolved conflict
    PrevConflict,
    /// Scroll down half a page, disabling cursor following
    ScrollDown,
    /// Scroll up half a page, disabling cursor following
    ScrollUp,
    /// Scroll all three panes left for long lines
    ScrollLeft,
    /// Scroll all three panes right for long lines
    ScrollRight,
    /// Copy the current chunk's result
    CopyChunk,
    /// Copy the entire file's result
    CopyFile,
    /// Copy the current chunk's local side
    CopyLocal,
    /// Copy the current chunk's remote side
    CopyRemote,
    /// Write the current file
    WriteFile,
    /// Switch to the next file
    NextFile,
    /// Fold or unfold stable regions
    ToggleFold,
    /// Quit the session
    Quit,
    /// Open the help overlay
    Help,
}

/// Kebab-case action names used in `[keys]` configuration.
static ACTION_NAMES: &[(&str, Action)] = &[
    ("take-local", Action::TakeLocal),
    ("take-remote", Action::TakeRemote),
    ("ignore", Action::IgnoreChunk),
    ("undo", Action::UndoChunk),
    ("undo-file", Action::UndoFile),
    ("edit", Action::EditChunk),
    ("apply-all", Action::ApplyNonConflict),
    ("next-change", Action::NextChange),
    ("prev-change", Action::PrevChange),
    ("next-conflict", Action::NextConflict),
    ("prev-conflict", Action::PrevConflict),
    ("scroll-down", Action::ScrollDown),
    ("scroll-up", Action::ScrollUp),
    ("scroll-left", Action::ScrollLeft),
    ("scroll-right", Action::ScrollRight),
    ("copy-chunk", Action::CopyChunk),
    ("copy-file", Action::CopyFile),
    ("copy-local", Action::CopyLocal),
    ("copy-remote", Action::CopyRemote),
    ("write", Action::WriteFile),
    ("next-file", Action::NextFile),
    ("fold", Action::ToggleFold),
    ("quit", Action::Quit),
    ("help", Action::Help),
];

/// Default binding group displayed as one row, such as `u / U`, with multiple dispatch keys.
struct Binding {
    /// Identifying action referenced by layout lists
    id: Action,
    /// Keys with modifiers mapped to actions, such as chunk undo (u) and file undo (U)
    keys: &'static [(KeyCode, KeyModifiers, Action)],
    /// Hint-bar message key; None excludes this group from the hint bar
    hint: Option<&'static str>,
    /// Help-overlay message key
    help: &'static str,
}

/// All default bindings; display labels are derived from keys rather than hardcoded.
static BINDINGS: &[Binding] = &[
    Binding {
        id: Action::TakeLocal,
        keys: &[
            (KeyCode::Char('h'), KeyModifiers::NONE, Action::TakeLocal),
            (KeyCode::Left, KeyModifiers::NONE, Action::TakeLocal),
        ],
        hint: Some("ui.hint_left"),
        help: "ui.help_left",
    },
    Binding {
        id: Action::TakeRemote,
        keys: &[
            (KeyCode::Char('l'), KeyModifiers::NONE, Action::TakeRemote),
            (KeyCode::Right, KeyModifiers::NONE, Action::TakeRemote),
        ],
        hint: Some("ui.hint_right"),
        help: "ui.help_right",
    },
    Binding {
        id: Action::IgnoreChunk,
        keys: &[(KeyCode::Char('x'), KeyModifiers::NONE, Action::IgnoreChunk)],
        hint: Some("ui.hint_ignore"),
        help: "ui.help_ignore",
    },
    Binding {
        id: Action::UndoChunk,
        keys: &[
            (KeyCode::Char('u'), KeyModifiers::NONE, Action::UndoChunk),
            (KeyCode::Char('U'), KeyModifiers::NONE, Action::UndoFile),
        ],
        hint: Some("ui.hint_undo"),
        help: "ui.help_undo",
    },
    Binding {
        id: Action::EditChunk,
        keys: &[(KeyCode::Char('e'), KeyModifiers::NONE, Action::EditChunk)],
        hint: Some("ui.hint_edit"),
        help: "ui.help_edit",
    },
    Binding {
        id: Action::ApplyNonConflict,
        keys: &[(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            Action::ApplyNonConflict,
        )],
        hint: None,
        help: "ui.help_apply",
    },
    Binding {
        id: Action::WriteFile,
        keys: &[(KeyCode::Char('w'), KeyModifiers::NONE, Action::WriteFile)],
        hint: Some("ui.hint_write"),
        help: "ui.help_write",
    },
    Binding {
        id: Action::Quit,
        keys: &[(KeyCode::Char('q'), KeyModifiers::NONE, Action::Quit)],
        hint: Some("ui.hint_quit"),
        help: "ui.help_quit",
    },
    Binding {
        id: Action::NextChange,
        keys: &[
            (KeyCode::Char('j'), KeyModifiers::NONE, Action::NextChange),
            (KeyCode::Down, KeyModifiers::NONE, Action::NextChange),
            (KeyCode::Char('k'), KeyModifiers::NONE, Action::PrevChange),
            (KeyCode::Up, KeyModifiers::NONE, Action::PrevChange),
        ],
        hint: None,
        help: "ui.help_move",
    },
    Binding {
        id: Action::NextConflict,
        keys: &[
            (KeyCode::Char('n'), KeyModifiers::NONE, Action::NextConflict),
            (KeyCode::Char('p'), KeyModifiers::NONE, Action::PrevConflict),
        ],
        hint: Some("ui.hint_conflict"),
        help: "ui.help_jump",
    },
    Binding {
        id: Action::ScrollDown,
        keys: &[
            (
                KeyCode::Char('d'),
                KeyModifiers::CONTROL,
                Action::ScrollDown,
            ),
            (KeyCode::Char('u'), KeyModifiers::CONTROL, Action::ScrollUp),
        ],
        hint: None,
        help: "ui.help_scroll",
    },
    Binding {
        id: Action::ScrollLeft,
        keys: &[
            (KeyCode::Char('<'), KeyModifiers::NONE, Action::ScrollLeft),
            (KeyCode::Char('>'), KeyModifiers::NONE, Action::ScrollRight),
            (KeyCode::Left, KeyModifiers::SHIFT, Action::ScrollLeft),
            (KeyCode::Right, KeyModifiers::SHIFT, Action::ScrollRight),
        ],
        hint: None,
        help: "ui.help_hscroll",
    },
    Binding {
        id: Action::NextFile,
        keys: &[(KeyCode::Tab, KeyModifiers::NONE, Action::NextFile)],
        hint: Some("ui.hint_file"),
        help: "ui.help_tab",
    },
    Binding {
        id: Action::ToggleFold,
        keys: &[(KeyCode::Char('z'), KeyModifiers::NONE, Action::ToggleFold)],
        hint: Some("ui.hint_fold"),
        help: "ui.help_fold",
    },
    Binding {
        id: Action::CopyChunk,
        keys: &[(KeyCode::Char('y'), KeyModifiers::NONE, Action::CopyChunk)],
        hint: None,
        help: "ui.help_copy_chunk",
    },
    Binding {
        id: Action::CopyFile,
        keys: &[(KeyCode::Char('Y'), KeyModifiers::NONE, Action::CopyFile)],
        hint: None,
        help: "ui.help_copy_file",
    },
    Binding {
        id: Action::CopyLocal,
        keys: &[
            (KeyCode::Char('H'), KeyModifiers::NONE, Action::CopyLocal),
            (KeyCode::Char('L'), KeyModifiers::NONE, Action::CopyRemote),
        ],
        hint: None,
        help: "ui.help_copy_sides",
    },
    Binding {
        id: Action::Help,
        keys: &[(KeyCode::Char('?'), KeyModifiers::NONE, Action::Help)],
        hint: Some("ui.hint_help"),
        help: "ui.help_help",
    },
];

/// Hint-bar display order, referencing each group's identifying action.
static HINT_LAYOUT: &[Action] = &[
    Action::TakeLocal,
    Action::TakeRemote,
    Action::IgnoreChunk,
    Action::UndoChunk,
    Action::EditChunk,
    Action::NextConflict,
    Action::WriteFile,
    Action::NextFile,
    Action::ToggleFold,
    Action::Quit,
    Action::Help,
];

/// Display order for the help overlay's left column.
static HELP_LEFT: &[Action] = &[
    Action::TakeLocal,
    Action::TakeRemote,
    Action::IgnoreChunk,
    Action::UndoChunk,
    Action::EditChunk,
    Action::ApplyNonConflict,
    Action::WriteFile,
    Action::Quit,
];

/// Display order for the help overlay's right column.
static HELP_RIGHT: &[Action] = &[
    Action::NextChange,
    Action::NextConflict,
    Action::ScrollDown,
    Action::ScrollLeft,
    Action::NextFile,
    Action::ToggleFold,
    Action::CopyChunk,
    Action::CopyFile,
    Action::CopyLocal,
    Action::Help,
];

/// Effective binding group combining defaults and `[keys]` overrides, with labels derived from keys.
#[derive(Debug)]
struct EffectiveBinding {
    /// Group's identifying action
    id: Action,
    /// Keys with modifiers mapped to actions
    keys: Vec<(KeyCode, KeyModifiers, Action)>,
    /// Help-overlay key label, such as `u / U`
    label: String,
    /// Compact hint-bar key label, such as `u/U`
    hint_label: String,
    /// Hint-bar message key
    hint: Option<&'static str>,
    /// Help-overlay message key
    help: &'static str,
}

/// Process-wide effective bindings, frozen by init or lazily defaulted when uninitialized.
static EFFECTIVE: OnceLock<Vec<EffectiveBinding>> = OnceLock::new();

/// Apply `[keys]` overrides and freeze the bindings before entering any TUI.
/// Only the first call takes effect within the process.
pub(crate) fn init(overrides: &HashMap<String, String>) -> Result<()> {
    let table = build(overrides)?;
    let _ = EFFECTIVE.set(table);
    Ok(())
}

/// Get effective bindings, building infallible defaults if init has not run.
fn effective() -> &'static [EffectiveBinding] {
    EFFECTIVE.get_or_init(|| build(&HashMap::new()).unwrap_or_default())
}

/// Combine defaults with overrides, replacing all default keys for each overridden
/// action and checking the complete table for key conflicts.
fn build(overrides: &HashMap<String, String>) -> Result<Vec<EffectiveBinding>> {
    let mut parsed: HashMap<Action, (KeyCode, KeyModifiers)> = HashMap::new();
    for (name, desc) in overrides {
        let Some(&(_, action)) = ACTION_NAMES.iter().find(|(n, _)| n == name) else {
            let list: Vec<&str> = ACTION_NAMES.iter().map(|(n, _)| *n).collect();
            bail!(
                "{}",
                tr_f(
                    "config.bad_action",
                    &[("name", name), ("list", &list.join(", "))],
                )
            );
        };
        let Some(key) = parse_key(desc) else {
            bail!(
                "{}",
                tr_f("config.bad_key", &[("name", name), ("key", desc)])
            );
        };
        parsed.insert(action, key);
    }

    let mut table = Vec::with_capacity(BINDINGS.len());
    for binding in BINDINGS {
        let mut keys: Vec<(KeyCode, KeyModifiers, Action)> = Vec::new();
        let mut replaced: Vec<Action> = Vec::new();
        for (code, mods, action) in binding.keys {
            match parsed.get(action) {
                // Replace all default keys for this action with the configured key, inserted once.
                Some(&(new_code, new_mods)) => {
                    if !replaced.contains(action) {
                        keys.push((new_code, new_mods, *action));
                        replaced.push(*action);
                    }
                }
                None => keys.push((*code, *mods, *action)),
            }
        }
        let (label, hint_label) = derive_labels(&keys);
        table.push(EffectiveBinding {
            id: binding.id,
            keys,
            label,
            hint_label,
            hint: binding.hint,
            help: binding.help,
        });
    }

    let mut seen: HashSet<(KeyCode, KeyModifiers)> = HashSet::new();
    for (code, mods, _) in table.iter().flat_map(|b| &b.keys) {
        if !seen.insert((*code, *mods)) {
            bail!(
                "{}",
                tr_f(
                    "config.key_conflict",
                    &[("key", &key_display(*code, *mods, false))],
                )
            );
        }
    }
    Ok(table)
}

/// Parse case-sensitive characters or case-insensitive named keys (left / tab / f5 / space),
/// optionally prefixed with `ctrl+` / `alt+` / `shift+`. Normalize shifted letters
/// to uppercase characters; return None for invalid descriptions.
fn parse_key(desc: &str) -> Option<(KeyCode, KeyModifiers)> {
    let parts: Vec<&str> = desc.split('+').map(str::trim).collect();
    let (mod_parts, key_part) = parts.split_at(parts.len() - 1);
    let name = *key_part.first()?;
    if name.is_empty() {
        return None;
    }

    let mut mods = KeyModifiers::NONE;
    for part in mod_parts {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "alt" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            _ => return None,
        }
    }

    let lower = name.to_ascii_lowercase();
    let mut code = match lower.as_str() {
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "tab" => KeyCode::Tab,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        f if f.len() >= 2 && f.starts_with('f') && f[1..].chars().all(|c| c.is_ascii_digit()) => {
            let n: u8 = f[1..].parse().ok()?;
            if !(1..=12).contains(&n) {
                return None;
            }
            KeyCode::F(n)
        }
        _ => {
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            KeyCode::Char(c)
        }
    };
    // Fold SHIFT into uppercase letters; Char keys do not retain a separate SHIFT modifier.
    if mods.contains(KeyModifiers::SHIFT)
        && let KeyCode::Char(c) = code
    {
        code = KeyCode::Char(c.to_ascii_uppercase());
        mods -= KeyModifiers::SHIFT;
    }
    Some((code, mods))
}

/// Display name for one key; `compact` uses hint-bar symbols, including an arrow for Tab.
fn key_display(code: KeyCode, mods: KeyModifiers, compact: bool) -> String {
    let base = match code {
        KeyCode::Char(' ') => "Space".to_owned(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Left => "←".to_owned(),
        KeyCode::Right => "→".to_owned(),
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::Tab => if compact { "⇥" } else { "Tab" }.to_owned(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    let mut out = String::new();
    if mods.contains(KeyModifiers::CONTROL) {
        out.push_str("ctrl+");
    }
    if mods.contains(KeyModifiers::ALT) {
        out.push_str("alt+");
    }
    // SHIFT is encoded in Char keys; only named keys need a shift+ prefix.
    if mods.contains(KeyModifiers::SHIFT) && !matches!(code, KeyCode::Char(_)) {
        out.push_str("shift+");
    }
    out.push_str(&base);
    out
}

/// Derive (help label, compact hint label) from a group's keys.
/// Single-action groups list all keys; multi-action groups use each action's first key.
fn derive_labels(keys: &[(KeyCode, KeyModifiers, Action)]) -> (String, String) {
    let mut actions: Vec<Action> = Vec::new();
    for (_, _, action) in keys {
        if !actions.contains(action) {
            actions.push(*action);
        }
    }
    let first_key = |action: Action, compact: bool| -> Option<String> {
        keys.iter()
            .find(|(_, _, a)| *a == action)
            .map(|(c, m, _)| key_display(*c, *m, compact))
    };
    let label = if actions.len() == 1 {
        keys.iter()
            .map(|(c, m, _)| key_display(*c, *m, false))
            .collect::<Vec<_>>()
            .join(" / ")
    } else {
        actions
            .iter()
            .filter_map(|a| first_key(*a, false))
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let hint_label = actions
        .iter()
        .filter_map(|a| first_key(*a, true))
        .collect::<Vec<_>>()
        .join("/");
    (label, hint_label)
}

/// Resolve a key event to an action, or None if unbound.
/// Ignore SHIFT for Char keys because case is encoded in the character.
pub(crate) fn action_for(code: KeyCode, mods: KeyModifiers) -> Option<Action> {
    action_in(effective(), code, mods)
}

/// Look up a key in a table: the pure part of action_for, allowing override tests.
fn action_in(table: &[EffectiveBinding], code: KeyCode, mods: KeyModifiers) -> Option<Action> {
    let mods = match code {
        KeyCode::Char(_) => mods - KeyModifiers::SHIFT,
        _ => mods,
    };
    table
        .iter()
        .flat_map(|b| &b.keys)
        .find(|(key, m, _)| *key == code && *m == mods)
        .map(|(_, _, action)| *action)
}

/// Get an effective binding group by identifying action for layout references.
fn group(id: Action) -> Option<&'static EffectiveBinding> {
    effective().iter().find(|b| b.id == id)
}

/// Compact label for an action's first effective key, used in inline instructions
/// such as whole-file pick cards and automatically reflecting configuration overrides.
pub(crate) fn key_hint(action: Action) -> String {
    key_hint_in(effective(), action)
}

/// Get an action's first key label from a table: the pure, testable part of key_hint.
fn key_hint_in(table: &[EffectiveBinding], action: Action) -> String {
    table
        .iter()
        .flat_map(|b| &b.keys)
        .find(|(_, _, a)| *a == action)
        .map(|(code, mods, _)| key_display(*code, *mods, true))
        .unwrap_or_default()
}

/// Hint-bar entries as (compact key label, message key), in display order.
pub(crate) fn hint_entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    HINT_LAYOUT.iter().filter_map(|id| {
        let binding = group(*id)?;
        Some((binding.hint_label.as_str(), binding.hint?))
    })
}

/// Help entry: (key label, message key).
pub(crate) type HelpEntry = (&'static str, &'static str);

/// Entries for one help-overlay column.
fn help_entries(layout: &'static [Action]) -> Vec<HelpEntry> {
    layout
        .iter()
        .filter_map(|id| group(*id).map(|b| (b.label.as_str(), b.help)))
        .collect()
}

/// Entries for the help overlay's left and right columns.
pub(crate) fn help_columns() -> (Vec<HelpEntry>, Vec<HelpEntry>) {
    (help_entries(HELP_LEFT), help_entries(HELP_RIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default table built without overrides.
    fn default_table() -> Vec<EffectiveBinding> {
        build(&HashMap::new()).unwrap()
    }

    /// Common keys resolve as expected; unbound keys return None and Char keys ignore SHIFT.
    #[test]
    fn keys_resolve_to_expected_actions() {
        let t = default_table();
        let f = |code| action_in(&t, code, KeyModifiers::NONE);
        assert_eq!(f(KeyCode::Char('h')), Some(Action::TakeLocal));
        assert_eq!(f(KeyCode::Left), Some(Action::TakeLocal));
        assert_eq!(f(KeyCode::Char('U')), Some(Action::UndoFile));
        assert_eq!(f(KeyCode::Char('k')), Some(Action::PrevChange));
        assert_eq!(f(KeyCode::Tab), Some(Action::NextFile));
        assert_eq!(f(KeyCode::Char('?')), Some(Action::Help));
        assert_eq!(f(KeyCode::Char('0')), None);
        assert_eq!(f(KeyCode::Esc), None);
        // Scrolling requires CONTROL; plain d is unbound and plain u still undoes.
        assert_eq!(
            action_in(&t, KeyCode::Char('d'), KeyModifiers::CONTROL),
            Some(Action::ScrollDown)
        );
        assert_eq!(
            action_in(&t, KeyCode::Char('u'), KeyModifiers::CONTROL),
            Some(Action::ScrollUp)
        );
        assert_eq!(f(KeyCode::Char('d')), None);
        assert_eq!(f(KeyCode::Char('u')), Some(Action::UndoChunk));
        // SHIFT modifiers supplied by terminals for uppercase characters do not affect matching.
        assert_eq!(
            action_in(&t, KeyCode::Char('U'), KeyModifiers::SHIFT),
            Some(Action::UndoFile)
        );
        // Modifiers on non-Char keys must match exactly.
        assert_eq!(action_in(&t, KeyCode::Left, KeyModifiers::CONTROL), None);
    }

    /// Derived default labels match the existing UI to guard against visual regressions.
    #[test]
    fn default_labels_match_previous_ui() {
        let expected: [(&str, &str); 18] = [
            ("h / ←", "h"),
            ("l / →", "l"),
            ("x", "x"),
            ("u / U", "u/U"),
            ("e", "e"),
            ("a", "a"),
            ("w", "w"),
            ("q", "q"),
            ("j / k", "j/k"),
            ("n / p", "n/p"),
            ("ctrl+d / ctrl+u", "ctrl+d/ctrl+u"),
            ("< / >", "</>"),
            ("Tab", "⇥"),
            ("z", "z"),
            ("y", "y"),
            ("Y", "Y"),
            ("H / L", "H/L"),
            ("?", "?"),
        ];
        let table = default_table();
        assert_eq!(table.len(), expected.len());
        for (binding, (label, hint_label)) in table.iter().zip(expected) {
            assert_eq!(binding.label, label, "{:?} help label changed", binding.id);
            assert_eq!(
                binding.hint_label, hint_label,
                "{:?} hint-bar label changed",
                binding.id
            );
        }
    }

    /// Parse named keys, modifiers, shift normalization, and invalid descriptions.
    #[test]
    fn parse_key_descriptions() {
        assert_eq!(
            parse_key("h"),
            Some((KeyCode::Char('h'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("U"),
            Some((KeyCode::Char('U'), KeyModifiers::NONE))
        );
        assert_eq!(parse_key("LEFT"), Some((KeyCode::Left, KeyModifiers::NONE)));
        assert_eq!(
            parse_key("ctrl+s"),
            Some((KeyCode::Char('s'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("alt+enter"),
            Some((KeyCode::Enter, KeyModifiers::ALT))
        );
        assert_eq!(
            parse_key("shift+u"),
            Some((KeyCode::Char('U'), KeyModifiers::NONE))
        );
        assert_eq!(parse_key("f5"), Some((KeyCode::F(5), KeyModifiers::NONE)));
        assert_eq!(
            parse_key("space"),
            Some((KeyCode::Char(' '), KeyModifiers::NONE))
        );
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("ctrl+"), None);
        assert_eq!(parse_key("foo"), None);
        assert_eq!(parse_key("f13"), None);
        assert_eq!(parse_key("meta+x"), None);
    }

    /// [keys] overrides replace every default key for an action and update labels.
    #[test]
    fn overrides_replace_defaults_and_labels() {
        let overrides = HashMap::from([
            ("take-local".to_owned(), "o".to_owned()),
            ("write".to_owned(), "ctrl+s".to_owned()),
        ]);
        let table = build(&overrides).unwrap();
        assert_eq!(
            action_in(&table, KeyCode::Char('o'), KeyModifiers::NONE),
            Some(Action::TakeLocal)
        );
        // Both default keys, h and Left, have been replaced.
        assert_eq!(
            action_in(&table, KeyCode::Char('h'), KeyModifiers::NONE),
            None
        );
        assert_eq!(action_in(&table, KeyCode::Left, KeyModifiers::NONE), None);
        assert_eq!(
            action_in(&table, KeyCode::Char('s'), KeyModifiers::CONTROL),
            Some(Action::WriteFile)
        );
        let take_local = table.iter().find(|b| b.id == Action::TakeLocal).unwrap();
        assert_eq!(
            (take_local.label.as_str(), take_local.hint_label.as_str()),
            ("o", "o")
        );
        let write = table.iter().find(|b| b.id == Action::WriteFile).unwrap();
        assert_eq!(write.label, "ctrl+s");
    }

    /// Overriding one group action preserves the other's defaults and combines their labels.
    #[test]
    fn partial_group_override_keeps_sibling() {
        let overrides = HashMap::from([("undo".to_owned(), "ctrl+z".to_owned())]);
        let table = build(&overrides).unwrap();
        assert_eq!(
            action_in(&table, KeyCode::Char('z'), KeyModifiers::CONTROL),
            Some(Action::UndoChunk)
        );
        assert_eq!(
            action_in(&table, KeyCode::Char('U'), KeyModifiers::NONE),
            Some(Action::UndoFile)
        );
        let undo = table.iter().find(|b| b.id == Action::UndoChunk).unwrap();
        assert_eq!(undo.label, "ctrl+z / U");
    }

    /// Inline hints use the action's first key and follow configured overrides.
    #[test]
    fn key_hint_follows_overrides() {
        let defaults = default_table();
        assert_eq!(key_hint_in(&defaults, Action::TakeLocal), "h");
        assert_eq!(key_hint_in(&defaults, Action::TakeRemote), "l");
        // U belongs to UndoFile in the same group and must not enter UndoChunk's label.
        assert_eq!(key_hint_in(&defaults, Action::UndoChunk), "u");

        let overrides = HashMap::from([
            ("take-local".to_owned(), "o".to_owned()),
            ("write".to_owned(), "ctrl+s".to_owned()),
        ]);
        let table = build(&overrides).unwrap();
        assert_eq!(key_hint_in(&table, Action::TakeLocal), "o");
        assert_eq!(key_hint_in(&table, Action::WriteFile), "ctrl+s");
    }

    /// Conflicting overrides and invalid input produce readable errors.
    #[test]
    fn override_errors_are_readable() {
        let bad_action = HashMap::from([("no-such".to_owned(), "o".to_owned())]);
        let err = build(&bad_action).unwrap_err().to_string();
        assert!(err.contains("no-such") && err.contains("take-local"));

        let bad_key = HashMap::from([("write".to_owned(), "ctrl+".to_owned())]);
        let err = build(&bad_key).unwrap_err().to_string();
        assert!(err.contains("ctrl+") && err.contains("write"));

        // The override collides with another action's default key.
        let conflict = HashMap::from([("write".to_owned(), "h".to_owned())]);
        let err = build(&conflict).unwrap_err().to_string();
        assert!(err.contains('h'));
    }

    /// Layout references must identify existing groups; hint-bar groups require message keys.
    #[test]
    fn layouts_reference_existing_groups() {
        for id in HINT_LAYOUT {
            let binding = group(*id).expect("hint bar references a missing binding group");
            assert!(
                binding.hint.is_some(),
                "{id:?} is in the hint bar but has no hint message"
            );
        }
        for id in HELP_LEFT.iter().chain(HELP_RIGHT) {
            assert!(
                group(*id).is_some(),
                "help overlay references a missing binding group"
            );
        }
        // Layout regression guard: 11 hint entries and 8 / 10 help entries on the left / right.
        assert_eq!(hint_entries().count(), 11);
        let (left, right) = help_columns();
        assert_eq!((left.len(), right.len()), (8, 10));
    }

    /// Each group's identifying action must be reachable from its keys to keep layout references valid.
    #[test]
    fn group_ids_are_reachable_from_own_keys() {
        for binding in BINDINGS {
            assert!(
                binding
                    .keys
                    .iter()
                    .any(|(_, _, action)| *action == binding.id),
                "group {:?} has no key for its identifying action",
                binding.id
            );
        }
    }
}
