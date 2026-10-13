//! Runtime language selection and message lookup.
//!
//! Message catalogs live in `locales/*.conf` (embedded at compile time via
//! `include_str!`). Lookup falls back from the active language to English,
//! then to the key itself, so a missing translation can never panic.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Supported UI languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Chinese
    Zh,
    /// English
    En,
}

/// English catalog (the reference set of all keys).
const EN: &str = include_str!("../locales/en.conf");
/// Chinese catalog.
const ZH: &str = include_str!("../locales/zh.conf");

/// Active language; defaults to English before initialization.
static LANG: OnceLock<Lang> = OnceLock::new();

/// Set the UI language; only the first call takes effect, so initialize early.
pub fn init(lang: Lang) {
    let _ = LANG.set(lang);
}

/// Detect the system language: Chinese for a zh locale prefix, English otherwise.
pub fn detect() -> Lang {
    match sys_locale::get_locale() {
        Some(l) if l.to_ascii_lowercase().starts_with("zh") => Lang::Zh,
        _ => Lang::En,
    }
}

/// Look up a message, falling back to English and then to the key itself.
pub fn tr(key: &'static str) -> &'static str {
    let lang = LANG.get().copied().unwrap_or(Lang::En);
    if lang == Lang::Zh
        && let Some(v) = table(Lang::Zh).get(key)
    {
        return v;
    }
    table(Lang::En).get(key).map_or(key, String::as_str)
}

/// Look up a message and replace named placeholders using `(name, value)` pairs.
/// Each `{name}` in the message is replaced in sequence.
pub fn tr_f(key: &'static str, args: &[(&str, &str)]) -> String {
    let mut text = tr(key).to_owned();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// Get a language's message table, parsed once per language in the process.
fn table(lang: Lang) -> &'static HashMap<&'static str, String> {
    static EN_TABLE: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    static ZH_TABLE: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    match lang {
        Lang::En => EN_TABLE.get_or_init(|| parse(EN)),
        Lang::Zh => ZH_TABLE.get_or_init(|| parse(ZH)),
    }
}

/// Parse conf messages: `key = value` lines, `#` comments, and `\n` newline escapes.
fn parse(src: &'static str) -> HashMap<&'static str, String> {
    let mut map = HashMap::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        map.insert(key.trim(), value.trim().replace("\\n", "\n"));
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse comments, empty lines, values containing =, and newline escapes.
    #[test]
    fn parses_conf_lines() {
        let map = parse("# comment\n\na.b = hello = world\nc = line1\\nline2\n");
        assert_eq!(map["a.b"], "hello = world");
        assert_eq!(map["c"], "line1\nline2");
        assert_eq!(map.len(), 2);
    }

    /// English and Chinese key sets must match; report missing translations.
    #[test]
    fn catalogs_have_identical_keys() {
        let en: std::collections::BTreeSet<_> = table(Lang::En).keys().collect();
        let zh: std::collections::BTreeSet<_> = table(Lang::Zh).keys().collect();
        let only_en: Vec<_> = en.difference(&zh).collect();
        let only_zh: Vec<_> = zh.difference(&en).collect();
        assert!(
            only_en.is_empty() && only_zh.is_empty(),
            "English-only keys: {only_en:?}; Chinese-only keys: {only_zh:?}"
        );
    }

    /// Both catalogs must have matching placeholders to avoid missing arguments.
    #[test]
    fn placeholders_match_between_catalogs() {
        let holes = |s: &str| -> std::collections::BTreeSet<String> {
            let mut set = std::collections::BTreeSet::new();
            let mut rest = s;
            while let Some(start) = rest.find('{') {
                if let Some(len) = rest[start..].find('}') {
                    set.insert(rest[start..start + len + 1].to_owned());
                    rest = &rest[start + len + 1..];
                } else {
                    break;
                }
            }
            set
        };
        for (key, en_val) in table(Lang::En) {
            let zh_val = &table(Lang::Zh)[key];
            assert_eq!(
                holes(en_val),
                holes(zh_val),
                "placeholders for key {key} differ between catalogs"
            );
        }
    }

    /// Fallback behavior before initialization and for missing keys.
    #[test]
    fn lookup_falls_back() {
        assert_eq!(tr("no.such.key"), "no.such.key");
        assert_eq!(tr("menu.no_output"), "(no output)");
        assert_eq!(tr_f("menu.failed", &[("cmd", "pull")]), "git pull failed");
    }
}
