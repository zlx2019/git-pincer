//! 高亮计算与缓存:块内词级差异(delta 式 word-level emphasis)+ syntect 语法高亮。
//!
//! 词级差异分两阶段:先在块内做行级配对(Replace 区间的新旧行按下标一一配对,
//! 与 delta 同款策略),再对每对行做行内词级 diff 得到变化的字节区间。
//! 三方模式下各侧与 base 对比;`file` 模式解析出的冲突块 base 恒为空,
//! 此时退化为 ours ↔ theirs 互比,两栏各自高亮「与对侧不同的词」。
//!
//! 语法高亮按文件计算一次而非逐帧:ours / theirs 栏内容不可变,构建后终身有效;
//! result 栏随取用变化,以 `revision` 判定失效并整栏重算(syntect 的解析状态
//! 跨行传递,无法按块独立重算)。只取 syntect 的前景色,背景让位给色带。

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use ratatui::style::Color;
use similar::utils::diff_words;
use similar::{Algorithm, ChangeTag, DiffTag, capture_diff_slices};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SyntectColor, ScopeSelectors, StyleModifier, Theme as SyntectTheme, ThemeItem,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::app::FileMerge;
use crate::merge::{ChunkKind, MergeChunk};

/// 词级强调的降级阈值:任一侧行数超限的块直接跳过
const EMPH_MAX_LINES: usize = 400;
/// 单行字节数超限时跳过该行的词级 diff
const EMPH_MAX_LINE_BYTES: usize = 2000;
/// 语法高亮的降级阈值:三栏总行数超限的文件禁用 syntect(fancy-regex 较慢)
const SYNTAX_MAX_LINES: usize = 10_000;

/// 进程级共享的语法定义集(dump 反序列化仅一次)。
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// 进程级共享的 syntect 主题。
fn syntect_theme() -> &'static SyntectTheme {
    static THEME: OnceLock<SyntectTheme> = OnceLock::new();
    THEME.get_or_init(maple_dark)
}

/// Maple Dark 主题(https://github.com/subframe7536/vscode-theme-maple)。
///
/// syntect 内置主题集中没有该主题,这里把其 tokenColors 的核心 scope
/// 手工移植为代码内构建的 syntect Theme:只移植前景色(背景让位给色带),
/// 不移植粗斜体。默认前景与 VSCode 版一致,渲染层会将其交回终端默认色。
fn maple_dark() -> SyntectTheme {
    /// scope 选择器 → 前景色,取自 maple-dark-color-theme.json
    const TOKEN_COLORS: &[(&str, (u8, u8, u8))] = &[
        (
            "comment, punctuation.definition.comment",
            (0x99, 0x99, 0x99),
        ),
        (
            "punctuation, meta.brace, keyword.operator",
            (0xb8, 0xd7, 0xf9),
        ),
        ("keyword, storage, support.type.builtin", (0xd2, 0xcc, 0xff)),
        (
            "constant, support.constant, variable.language",
            (0xf0, 0xc0, 0xa8),
        ),
        ("constant.numeric", (0xd5, 0xf2, 0x88)),
        ("constant.language", (0xd2, 0xcc, 0xff)),
        ("string", (0xa4, 0xdf, 0xae)),
        ("constant.character.escape", (0x8f, 0xc7, 0xff)),
        (
            "entity.name.function, support.function, variable.function, \
             support.macro, entity.name.macro",
            (0x8f, 0xc7, 0xff),
        ),
        (
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
             entity.name.trait, entity.other.inherited-class, support.class, support.type",
            (0xf0, 0xc0, 0xa8),
        ),
        ("variable, variable.parameter", (0xee, 0xcf, 0xa0)),
        (
            "variable.other.member, meta.property-name, support.type.property-name",
            (0xde, 0xd6, 0xcf),
        ),
        ("entity.name.tag", (0xed, 0xab, 0xab)),
        ("entity.other.attribute-name", (0xee, 0xcf, 0xa0)),
        (
            "entity.name.namespace, keyword.control.import, keyword.other.import, \
             keyword.other.package",
            (0xe3, 0xcb, 0xeb),
        ),
        ("markup.heading, entity.name.section", (0xd2, 0xcc, 0xff)),
        ("markup.quote", (0xa1, 0xe8, 0xe5)),
        ("markup.raw, markup.inline.raw", (0xed, 0xab, 0xab)),
        ("markup.inserted", (0xa4, 0xdf, 0xae)),
        ("markup.deleted", (0xed, 0xab, 0xab)),
        ("markup.bold, markup.italic", (0xf0, 0xc0, 0xa8)),
        (
            "markup.underline.link, string.other.link",
            (0x8f, 0xc7, 0xff),
        ),
    ];

    let rgb = |(r, g, b): (u8, u8, u8)| SyntectColor { r, g, b, a: 0xff };
    let mut theme = SyntectTheme {
        name: Some("Maple Dark".to_owned()),
        ..SyntectTheme::default()
    };
    // editor.foreground(#cbd5e1);渲染层会跳过等于默认前景的 token
    theme.settings.foreground = Some(rgb((0xcb, 0xd5, 0xe1)));
    for (scopes, color) in TOKEN_COLORS {
        // scope 常量在编译期固定,解析失败直接跳过该条而非 panic
        let Ok(scope) = scopes.parse::<ScopeSelectors>() else {
            continue;
        };
        theme.scopes.push(ThemeItem {
            scope,
            style: StyleModifier {
                foreground: Some(rgb(*color)),
                background: None,
                font_style: None,
            },
        });
    }
    theme
}

/// 一行中发生变化的字节区间(升序、互不重叠,均落在 char 边界)。
pub(crate) type Emphasis = Vec<Range<usize>>;

/// 一个块的词级强调:与 chunk.ours / chunk.theirs 的行一一对应。
#[derive(Debug, Default)]
pub(crate) struct ChunkEmphasis {
    pub(crate) ours: Vec<Emphasis>,
    pub(crate) theirs: Vec<Emphasis>,
}

/// 一栏的语法高亮:下标 = 该栏文档行号 - 1,每行为 (前景色, 字节区间) 列表。
#[derive(Debug, Default)]
pub(crate) struct PaneSyntax {
    pub(crate) lines: Vec<Vec<(Color, Range<usize>)>>,
}

/// 一个文件的全部高亮信息。
#[derive(Debug)]
pub(crate) struct FileHighlight {
    /// 各块的词级强调(与 chunks 一一对应,构建一次终身有效)
    pub(crate) emphasis: Vec<ChunkEmphasis>,
    /// 本地栏语法高亮(内容不可变,构建一次)
    pub(crate) ours: Option<PaneSyntax>,
    /// 远端栏语法高亮(内容不可变,构建一次)
    pub(crate) theirs: Option<PaneSyntax>,
    /// 结果栏语法高亮(随取用变化,按 revision 失效重算)
    pub(crate) result: Option<PaneSyntax>,
    /// 匹配到的语法定义;None 表示该文件不做语法高亮
    syntax: Option<&'static SyntaxReference>,
    /// result 栏所对应的状态修订号
    result_rev: u64,
}

impl FileHighlight {
    /// 构建文件的完整高亮信息。
    fn build(merge: &FileMerge, rev: u64) -> Self {
        let emphasis = merge.chunks.iter().map(chunk_emphasis).collect();
        let syntax = find_syntax(merge);
        let highlight =
            |lines: &mut dyn Iterator<Item = &String>| syntax.map(|s| highlight_pane(lines, s));
        Self {
            emphasis,
            ours: highlight(&mut merge.chunks.iter().flat_map(|c| c.ours.iter())),
            theirs: highlight(&mut merge.chunks.iter().flat_map(|c| c.theirs.iter())),
            result: syntax.map(|s| highlight_result(merge, s)),
            syntax,
            result_rev: rev,
        }
    }
}

/// 按文件下标缓存高亮信息。
#[derive(Debug, Default)]
pub(crate) struct HighlightCache {
    files: HashMap<usize, FileHighlight>,
}

impl HighlightCache {
    /// 取当前文件的高亮信息:首次访问时构建;
    /// revision 变化(取用 / 撤销 / 编辑)时只重算结果栏。
    pub(crate) fn get(&mut self, file_idx: usize, merge: &FileMerge, rev: u64) -> &FileHighlight {
        let entry = self
            .files
            .entry(file_idx)
            .or_insert_with(|| FileHighlight::build(merge, rev));
        if entry.result_rev != rev {
            entry.result = entry.syntax.map(|s| highlight_result(merge, s));
            entry.result_rev = rev;
        }
        entry
    }
}

/// 按扩展名匹配语法定义;超大文件(三栏总行数超限)禁用。
fn find_syntax(merge: &FileMerge) -> Option<&'static SyntaxReference> {
    let total: usize = merge
        .chunks
        .iter()
        .map(|c| c.ours.len() + c.theirs.len() + c.base.len())
        .sum();
    if total > SYNTAX_MAX_LINES {
        return None;
    }
    let ext = std::path::Path::new(&merge.path).extension()?.to_str()?;
    let syntax = syntax_set().find_syntax_by_extension(ext)?;
    // Plain Text 没有任何着色规则,整栏统一主题默认色反而压暗正文,
    // 不如不做语法高亮,让正文用终端默认前景(更亮)
    (syntax.name != "Plain Text").then_some(syntax)
}

/// 逐行高亮一栏内容;syntect 解析状态跨行传递,必须按文档顺序整栏计算。
fn highlight_pane(
    lines: &mut dyn Iterator<Item = &String>,
    syntax: &SyntaxReference,
) -> PaneSyntax {
    let set = syntax_set();
    let theme = syntect_theme();
    // 等于主题默认前景的 token(普通标识符 / 标点)不产出着色段,
    // 交回终端默认前景,保持正文亮度;只有语义色 token(关键字 / 字符串等)上色
    let default_fg = theme.settings.foreground;
    let mut hl = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for line in lines {
        // newlines 版语法集要求行带换行符,产出区间时再剔除
        let with_nl = format!("{line}\n");
        let mut spans = Vec::new();
        if let Ok(regions) = hl.highlight_line(&with_nl, set) {
            let mut pos = 0usize;
            for (style, token) in regions {
                let end = pos + token.len();
                let clipped = end.min(line.len());
                if clipped > pos && Some(style.foreground) != default_fg {
                    let fg = style.foreground;
                    spans.push((Color::Rgb(fg.r, fg.g, fg.b), pos..clipped));
                }
                pos = end;
            }
        }
        out.push(spans);
    }
    PaneSyntax { lines: out }
}

/// 高亮结果栏的当前内容(按块拼接后的完整文档)。
fn highlight_result(merge: &FileMerge, syntax: &SyntaxReference) -> PaneSyntax {
    let lines: Vec<String> = (0..merge.chunks.len())
        .flat_map(|i| merge.current_content(i))
        .collect();
    highlight_pane(&mut lines.iter(), syntax)
}

/// 计算一个块的词级强调;稳定块与超大块返回全空。
fn chunk_emphasis(chunk: &MergeChunk) -> ChunkEmphasis {
    let mut out = ChunkEmphasis {
        ours: vec![Emphasis::new(); chunk.ours.len()],
        theirs: vec![Emphasis::new(); chunk.theirs.len()],
    };
    let oversized = chunk.base.len() > EMPH_MAX_LINES
        || chunk.ours.len() > EMPH_MAX_LINES
        || chunk.theirs.len() > EMPH_MAX_LINES;
    if chunk.kind == ChunkKind::Stable || oversized {
        return out;
    }
    if chunk.base.is_empty() {
        // base 为空(file 模式的冲突块,或纯插入):冲突块改为两侧互比,
        // 单侧纯插入无对照物,不做词级强调(色带已表意)
        if chunk.kind == ChunkKind::Conflict {
            cross_emphasis(chunk, &mut out);
        }
        return out;
    }
    if matches!(
        chunk.kind,
        ChunkKind::Ours | ChunkKind::Agree | ChunkKind::Conflict
    ) {
        side_emphasis(&chunk.base, &chunk.ours, &mut out.ours);
    }
    if matches!(
        chunk.kind,
        ChunkKind::Theirs | ChunkKind::Agree | ChunkKind::Conflict
    ) {
        side_emphasis(&chunk.base, &chunk.theirs, &mut out.theirs);
    }
    out
}

/// 一侧相对 base 的词级强调:行级配对后,对每对行做行内 diff(区间归属改动侧)。
fn side_emphasis(base: &[String], side: &[String], out: &mut [Emphasis]) {
    for op in capture_diff_slices(Algorithm::Myers, base, side) {
        if op.tag() != DiffTag::Replace {
            continue;
        }
        // Replace 的新旧行按下标配对,多出的行整行无词级强调
        for (b, s) in op.old_range().zip(op.new_range()) {
            if let Some(slot) = out.get_mut(s) {
                *slot = line_emphasis(&base[b], &side[s]);
            }
        }
    }
}

/// 冲突块 base 为空时的退化:ours ↔ theirs 互比,两侧各自记录差异区间。
fn cross_emphasis(chunk: &MergeChunk, out: &mut ChunkEmphasis) {
    for op in capture_diff_slices(Algorithm::Myers, &chunk.ours, &chunk.theirs) {
        if op.tag() != DiffTag::Replace {
            continue;
        }
        for (o, t) in op.old_range().zip(op.new_range()) {
            let (ours_line, theirs_line) = (&chunk.ours[o], &chunk.theirs[t]);
            if let Some(slot) = out.ours.get_mut(o) {
                *slot = line_emphasis(theirs_line, ours_line);
            }
            if let Some(slot) = out.theirs.get_mut(t) {
                *slot = line_emphasis(ours_line, theirs_line);
            }
        }
    }
}

/// 行内词级 diff:返回 new 侧中与 old 不同的字节区间(相邻区间合并)。
///
/// 游标只在 Equal 与 Insert 时前进(两者都是 new 侧的内容),
/// 因此区间必然落在 char 边界上,渲染层切片安全。
fn line_emphasis(old: &str, new: &str) -> Emphasis {
    if old.len() > EMPH_MAX_LINE_BYTES || new.len() > EMPH_MAX_LINE_BYTES {
        return Emphasis::new();
    }
    let mut ranges = Emphasis::new();
    let mut pos = 0usize;
    for (tag, token) in diff_words(Algorithm::Myers, old, new) {
        match tag {
            ChangeTag::Delete => {}
            ChangeTag::Equal => pos += token.len(),
            ChangeTag::Insert => {
                let next = pos + token.len();
                // 相邻区间合并,减少渲染 Span 数量
                match ranges.last_mut() {
                    Some(last) if last.end == pos => last.end = next,
                    _ => ranges.push(pos..next),
                }
                pos = next;
            }
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工构造块的辅助函数
    fn chunk(kind: ChunkKind, base: &[&str], ours: &[&str], theirs: &[&str]) -> MergeChunk {
        let v = |s: &[&str]| s.iter().map(|t| (*t).to_owned()).collect();
        MergeChunk {
            id: 0,
            kind,
            base: v(base),
            ours: v(ours),
            theirs: v(theirs),
            base_start: 1,
            ours_start: 1,
            theirs_start: 1,
        }
    }

    /// 三方模式:各侧相对 base 的变化词被精确定位
    #[test]
    fn emphasis_against_base() {
        let c = chunk(
            ChunkKind::Conflict,
            &["let a = 1;"],
            &["let a = 2;"],
            &["let b = 1;"],
        );
        let e = chunk_emphasis(&c);
        // ours 变化 token 为 "2;"(词级粒度含相邻标点),theirs 为 "b"
        assert_eq!(e.ours[0], vec![8..10]);
        assert_eq!(e.theirs[0], vec![4..5]);
    }

    /// base 为空的冲突块:退化为 ours ↔ theirs 互比
    #[test]
    fn empty_base_falls_back_to_cross_compare() {
        let c = chunk(ChunkKind::Conflict, &[], &["hello world"], &["hello rust"]);
        let e = chunk_emphasis(&c);
        assert_eq!(e.ours[0], vec![6..11]); // "world"
        assert_eq!(e.theirs[0], vec![6..10]); // "rust"
    }

    /// 超大块直接降级为全空强调
    #[test]
    fn oversized_chunk_degrades_to_empty() {
        let lines: Vec<String> = (0..=EMPH_MAX_LINES).map(|i| format!("l{i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let c = chunk(ChunkKind::Conflict, &["a"], &refs, &["b"]);
        let e = chunk_emphasis(&c);
        assert!(e.ours.iter().all(Vec::is_empty));
        assert!(e.theirs.iter().all(Vec::is_empty));
    }

    /// .rs 扩展名产出非空语法高亮;未知扩展名优雅降级为 None
    #[test]
    fn syntax_matches_extension_or_falls_back() {
        let mut cache = HighlightCache::default();
        let rs = FileMerge::from_three_way(
            "demo.rs".to_owned(),
            "fn main() {}\n",
            "fn main() { let a = 1; }\n",
            "fn main() {}\n",
        );
        let hl = cache.get(0, &rs, 0);
        let ours = hl.ours.as_ref().unwrap();
        assert!(ours.lines.iter().any(|l| !l.is_empty()));

        let unknown = FileMerge::from_three_way("demo.unknownext".to_owned(), "a\n", "b\n", "a\n");
        let hl = cache.get(1, &unknown, 0);
        assert!(hl.ours.is_none());
        assert!(hl.result.is_none());
    }

    /// revision 变化时结果栏重算(行数随取用内容变化)
    #[test]
    fn result_pane_rehighlights_on_revision_change() {
        let mut merge =
            FileMerge::from_three_way("demo.rs".to_owned(), "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        let mut cache = HighlightCache::default();
        let before = cache.get(0, &merge, 0).result.as_ref().unwrap().lines.len();
        // 冲突两侧都取用 → 结果行数 3 → 4
        merge.apply(crate::app::Side::Ours);
        merge.apply(crate::app::Side::Theirs);
        let after = cache.get(0, &merge, 1).result.as_ref().unwrap().lines.len();
        assert_eq!(before, 3);
        assert_eq!(after, 4);
    }

    /// 三栏总行数超限的文件禁用语法高亮
    #[test]
    fn oversized_file_disables_syntax() {
        let text: String = (0..4000).map(|i| format!("l{i}\n")).collect();
        let merge = FileMerge::from_three_way("big.rs".to_owned(), &text, &text, &text);
        let mut cache = HighlightCache::default();
        let hl = cache.get(0, &merge, 0);
        assert!(hl.ours.is_none());
        // 词级强调不受影响(稳定块本就为空)
        assert_eq!(hl.emphasis.len(), merge.chunks.len());
    }
}
