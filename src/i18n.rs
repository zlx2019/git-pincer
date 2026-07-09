//! Runtime language selection and message lookup.
//!
//! Message catalogs live in `locales/*.conf` (embedded at compile time via
//! `include_str!`). Lookup falls back from the active language to English,
//! then to the key itself, so a missing translation can never panic.

use std::collections::HashMap;
use std::sync::OnceLock;

/// 支持的界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// 中文
    Zh,
    /// 英文
    En,
}

/// 英文文案(基准,key 全集)。
const EN: &str = include_str!("../locales/en.conf");
/// 中文文案。
const ZH: &str = include_str!("../locales/zh.conf");

/// 当前语言;未初始化时按英文处理。
static LANG: OnceLock<Lang> = OnceLock::new();

/// 设定界面语言(进程内仅首次调用生效,应在入口尽早调用)。
pub fn init(lang: Lang) {
    let _ = LANG.set(lang);
}

/// 探测系统语言:locale 为 zh 前缀(如 zh-CN)时用中文,其余用英文。
pub fn detect() -> Lang {
    match sys_locale::get_locale() {
        Some(l) if l.to_ascii_lowercase().starts_with("zh") => Lang::Zh,
        _ => Lang::En,
    }
}

/// 取当前语言的文案:当前语言缺失回退英文,再缺失返回 key 本身。
pub fn tr(key: &'static str) -> &'static str {
    let lang = LANG.get().copied().unwrap_or(Lang::En);
    if lang == Lang::Zh
        && let Some(v) = table(Lang::Zh).get(key)
    {
        return v;
    }
    table(Lang::En).get(key).map_or(key, String::as_str)
}

/// 取文案并替换命名占位符:`args` 为 `(名称, 值)` 列表,
/// 文案中的 `{名称}` 会被逐个替换。
pub fn tr_f(key: &'static str, args: &[(&str, &str)]) -> String {
    let mut text = tr(key).to_owned();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// 语言对应的文案表(进程内每种语言只解析一次)。
fn table(lang: Lang) -> &'static HashMap<&'static str, String> {
    static EN_TABLE: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    static ZH_TABLE: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    match lang {
        Lang::En => EN_TABLE.get_or_init(|| parse(EN)),
        Lang::Zh => ZH_TABLE.get_or_init(|| parse(ZH)),
    }
}

/// 解析 conf 文案:每行 `key = value`,`#` 注释,`\n` 转义为换行。
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

    /// conf 解析:注释、空行、含 = 的 value、\n 转义
    #[test]
    fn parses_conf_lines() {
        let map = parse("# comment\n\na.b = hello = world\nc = line1\\nline2\n");
        assert_eq!(map["a.b"], "hello = world");
        assert_eq!(map["c"], "line1\nline2");
        assert_eq!(map.len(), 2);
    }

    /// en 与 zh 的 key 集合必须完全一致,漏翻直接报出差集
    #[test]
    fn catalogs_have_identical_keys() {
        let en: std::collections::BTreeSet<_> = table(Lang::En).keys().collect();
        let zh: std::collections::BTreeSet<_> = table(Lang::Zh).keys().collect();
        let only_en: Vec<_> = en.difference(&zh).collect();
        let only_zh: Vec<_> = zh.difference(&en).collect();
        assert!(
            only_en.is_empty() && only_zh.is_empty(),
            "en 独有: {only_en:?}; zh 独有: {only_zh:?}"
        );
    }

    /// 两份文案的占位符集合必须一致(防止翻译丢参数)
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
                "key {key} 的占位符在两份文案中不一致"
            );
        }
    }

    /// 未初始化 / 缺失 key 的回退行为
    #[test]
    fn lookup_falls_back() {
        assert_eq!(tr("no.such.key"), "no.such.key");
        assert_eq!(tr("menu.no_output"), "(no output)");
        assert_eq!(tr_f("menu.failed", &[("cmd", "pull")]), "git pull failed");
    }
}
