//! 高亮计算与缓存:块内词级差异(delta 式 word-level emphasis)+ syntect 语法高亮。
//!
//! 词级差异分两阶段:先在块内做行级配对(Replace 区间的新旧行按下标一一配对,
//! 与 delta 同款策略),再对每对行做行内词级 diff 得到变化的字节区间。
//! 三方模式下各侧与 base 对比;`file` 模式解析出的冲突块 base 恒为空,
//! 此时退化为 ours ↔ theirs 互比,两栏各自高亮「与对侧不同的词」。
//!
//! 语法高亮按文件计算一次而非逐帧:ours / theirs 栏内容不可变,构建后终身有效;
//! result 栏随取用变化,以 `revision` 判定失效并按块增量重算(块边界缓存
//! syntect 解析状态,见 [`ResultSyntax`])。只取 syntect 的前景色,背景让位给色带。

use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use ratatui::style::Color;
use similar::utils::diff_words;
use similar::{Algorithm, ChangeTag, DiffTag, capture_diff_slices};
use syntect::highlighting::{
    Color as SyntectColor, HighlightIterator, HighlightState, Highlighter, ScopeSelectors,
    StyleModifier, Theme as SyntectTheme, ThemeItem,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::app::{ChunkState, FileMerge};
use crate::merge::{ChunkKind, MergeChunk};

/// 词级强调的降级阈值:任一侧行数超限的块直接跳过
const EMPH_MAX_LINES: usize = 400;
/// 单行字节数超限时跳过该行的词级 diff
const EMPH_MAX_LINE_BYTES: usize = 2000;
/// 词级强调的占比上限(%):强调字节超过行长的该比例时退化为整行色带
const EMPH_MAX_RATIO: usize = 70;
/// 语法高亮的降级阈值:三栏总行数超限的文件禁用 syntect(fancy-regex 较慢)
const SYNTAX_MAX_LINES: usize = 10_000;

/// 进程级共享的语法定义集(dump 反序列化仅一次)。
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// 进程级共享的 syntect 主题(深 / 浅两套,各自惰性构建一次)。
fn syntect_theme(light: bool) -> &'static SyntectTheme {
    static DARK: OnceLock<SyntectTheme> = OnceLock::new();
    static LIGHT: OnceLock<SyntectTheme> = OnceLock::new();
    if light {
        LIGHT.get_or_init(|| maple(true))
    } else {
        DARK.get_or_init(|| maple(false))
    }
}

/// RGB 三元组(语法配色表用)。
type Rgb = (u8, u8, u8);

/// scope 选择器 → (Maple Dark 前景色, Maple Light 前景色),
/// 取自 maple-{dark,light}-color-theme.json 的 tokenColors。
const TOKEN_COLORS: &[(&str, Rgb, Rgb)] = &[
    (
        "comment, punctuation.definition.comment",
        (0x99, 0x99, 0x99),
        (0x80, 0x80, 0x80),
    ),
    (
        "punctuation, meta.brace, keyword.operator",
        (0xb8, 0xd7, 0xf9),
        (0x71, 0xa3, 0xa8),
    ),
    (
        "keyword, storage, support.type.builtin",
        (0xd2, 0xcc, 0xff),
        (0x72, 0x62, 0x93),
    ),
    (
        "constant, support.constant, variable.language",
        (0xf0, 0xc0, 0xa8),
        (0xc3, 0x75, 0x22),
    ),
    ("constant.numeric", (0xd5, 0xf2, 0x88), (0x73, 0x99, 0x00)),
    ("constant.language", (0xd2, 0xcc, 0xff), (0x72, 0x62, 0x93)),
    ("string", (0xa4, 0xdf, 0xae), (0x47, 0x8f, 0x14)),
    (
        "constant.character.escape",
        (0x8f, 0xc7, 0xff),
        (0x05, 0x85, 0xa8),
    ),
    (
        "entity.name.function, support.function, variable.function, \
         support.macro, entity.name.macro",
        (0x8f, 0xc7, 0xff),
        (0x05, 0x85, 0xa8),
    ),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
         entity.name.trait, entity.other.inherited-class, support.class, support.type",
        (0xf0, 0xc0, 0xa8),
        (0xc3, 0x75, 0x22),
    ),
    (
        "variable, variable.parameter",
        (0xee, 0xcf, 0xa0),
        (0xaa, 0x83, 0x0e),
    ),
    (
        "variable.other.member, meta.property-name, support.type.property-name",
        (0xde, 0xd6, 0xcf),
        (0x8d, 0x89, 0x49),
    ),
    ("entity.name.tag", (0xed, 0xab, 0xab), (0xbd, 0x51, 0x51)),
    (
        "entity.other.attribute-name",
        (0xee, 0xcf, 0xa0),
        (0xaa, 0x83, 0x0e),
    ),
    (
        "entity.name.namespace, keyword.control.import, keyword.other.import, \
         keyword.other.package",
        (0xe3, 0xcb, 0xeb),
        (0xa6, 0x59, 0x73),
    ),
    (
        "markup.heading, entity.name.section",
        (0xd2, 0xcc, 0xff),
        (0x72, 0x62, 0x93),
    ),
    ("markup.quote", (0xa1, 0xe8, 0xe5), (0x12, 0x7d, 0x52)),
    (
        "markup.raw, markup.inline.raw",
        (0xed, 0xab, 0xab),
        (0xbd, 0x51, 0x51),
    ),
    ("markup.inserted", (0xa4, 0xdf, 0xae), (0x47, 0x8f, 0x14)),
    ("markup.deleted", (0xed, 0xab, 0xab), (0xbd, 0x51, 0x51)),
    (
        "markup.bold, markup.italic",
        (0xf0, 0xc0, 0xa8),
        (0xc3, 0x75, 0x22),
    ),
    (
        "markup.underline.link, string.other.link",
        (0x8f, 0xc7, 0xff),
        (0x05, 0x85, 0xa8),
    ),
];

/// Maple 主题(https://github.com/subframe7536/vscode-theme-maple)。
///
/// syntect 内置主题集中没有该主题,这里把其 tokenColors 的核心 scope
/// 手工移植为代码内构建的 syntect Theme:只移植前景色(背景让位给色带),
/// 不移植粗斜体。默认前景与 VSCode 版一致,渲染层会将其交回终端默认色。
fn maple(light: bool) -> SyntectTheme {
    let rgb = |(r, g, b): Rgb| SyntectColor { r, g, b, a: 0xff };
    let mut theme = SyntectTheme {
        name: Some(if light { "Maple Light" } else { "Maple Dark" }.to_owned()),
        ..SyntectTheme::default()
    };
    // editor.foreground(dark #cbd5e1 / light #475569);渲染层会跳过等于默认前景的 token
    theme.settings.foreground = Some(rgb(if light {
        (0x47, 0x55, 0x69)
    } else {
        (0xcb, 0xd5, 0xe1)
    }));
    for (scopes, dark, light_color) in TOKEN_COLORS {
        // scope 常量在编译期固定,解析失败直接跳过该条而非 panic
        let Ok(scope) = scopes.parse::<ScopeSelectors>() else {
            continue;
        };
        theme.scopes.push(ThemeItem {
            scope,
            style: StyleModifier {
                foreground: Some(rgb(if light { *light_color } else { *dark })),
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
    /// 结果栏语法高亮(随取用变化,按 revision 增量重算)
    pub(crate) result: Option<ResultSyntax>,
    /// 匹配到的语法定义;None 表示该文件不做语法高亮
    syntax: Option<&'static SyntaxReference>,
    /// 是否使用浅色语法主题(result 栏重算时沿用)
    light: bool,
    /// result 栏所对应的状态修订号
    result_rev: u64,
}

impl FileHighlight {
    /// 构建文件的完整高亮信息。
    fn build(merge: &FileMerge, rev: u64, light: bool) -> Self {
        let emphasis = merge.chunks.iter().map(chunk_emphasis).collect();
        let syntax = find_syntax(merge);
        let highlight = |lines: &mut dyn Iterator<Item = &String>| {
            syntax.map(|s| highlight_pane(lines, s, light))
        };
        Self {
            emphasis,
            ours: highlight(&mut merge.chunks.iter().flat_map(|c| c.ours_lines().iter())),
            theirs: highlight(&mut merge.chunks.iter().flat_map(|c| c.theirs_lines().iter())),
            result: syntax.map(|s| {
                let mut result = ResultSyntax::default();
                result.update(merge, s, light);
                result
            }),
            syntax,
            light,
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
    /// revision 变化(取用 / 撤销 / 编辑)时只增量重算结果栏的变化块。
    pub(crate) fn get(
        &mut self,
        file_idx: usize,
        merge: &FileMerge,
        rev: u64,
        light: bool,
    ) -> &FileHighlight {
        let entry = self
            .files
            .entry(file_idx)
            .or_insert_with(|| FileHighlight::build(merge, rev, light));
        if entry.result_rev != rev {
            if let (Some(result), Some(syntax)) = (entry.result.as_mut(), entry.syntax) {
                result.update(merge, syntax, entry.light);
            }
            entry.result_rev = rev;
        }
        entry
    }
}

/// 结果栏语法高亮:按块缓存边界解析状态与输出。
///
/// syntect 的解析状态跨行传递,无法按块独立计算;但可以在块边界快照状态:
/// 取用 / 撤销时从第一个内容变化的块开始重算,一旦后续块的入口状态与
/// 缓存一致即恢复命中(状态收敛),把单次按键的成本从 O(全文件) 降到
/// O(变化块 + 收敛尾巴)。
#[derive(Debug, Default)]
pub(crate) struct ResultSyntax {
    /// 与 chunks 一一对应的块级缓存
    chunks: Vec<ChunkSyntax>,
    /// 最近一次 update 实际解析的行数(增量效果的观测口)
    parsed_lines: usize,
}

/// 结果栏单个块的高亮缓存。
#[derive(Debug)]
struct ChunkSyntax {
    /// 块解决状态的指纹(内容仅由取用顺序与覆写决定)
    fingerprint: u64,
    /// 进入该块时的解析 / 高亮状态(命中判定用)
    start: (ParseState, HighlightState),
    /// 离开该块时的状态(命中时快进用)
    end: (ParseState, HighlightState),
    /// 块内每行的着色段
    spans: Vec<Vec<(Color, Range<usize>)>>,
}

impl ResultSyntax {
    /// 增量重算:逐块走一遍,指纹与入口状态都命中的块直接复用并快进状态。
    fn update(&mut self, merge: &FileMerge, syntax: &SyntaxReference, light: bool) {
        let set = syntax_set();
        let theme = syntect_theme(light);
        let highlighter = Highlighter::new(theme);
        let default_fg = theme.settings.foreground;
        let mut parse = ParseState::new(syntax);
        let mut hl = HighlightState::new(&highlighter, ScopeStack::new());
        self.parsed_lines = 0;

        for idx in 0..merge.chunks.len() {
            let fingerprint = state_fingerprint(&merge.states[idx]);
            if let Some(cached) = self.chunks.get(idx)
                && cached.fingerprint == fingerprint
                && cached.start.0 == parse
                && cached.start.1 == hl
            {
                parse = cached.end.0.clone();
                hl = cached.end.1.clone();
                continue;
            }
            let start = (parse.clone(), hl.clone());
            let lines = merge.current_content(idx);
            let mut spans = Vec::with_capacity(lines.len());
            for line in &lines {
                spans.push(line_spans(
                    &mut parse,
                    &mut hl,
                    &highlighter,
                    default_fg,
                    line,
                    set,
                ));
                self.parsed_lines += 1;
            }
            let entry = ChunkSyntax {
                fingerprint,
                start,
                end: (parse.clone(), hl.clone()),
                spans,
            };
            match self.chunks.get_mut(idx) {
                Some(slot) => *slot = entry,
                None => self.chunks.push(entry),
            }
        }
        self.chunks.truncate(merge.chunks.len());
    }

    /// 取某块内某行(块内偏移)的着色段。
    pub(crate) fn spans(&self, chunk: usize, offset: usize) -> &[(Color, Range<usize>)] {
        self.chunks
            .get(chunk)
            .and_then(|c| c.spans.get(offset))
            .map_or(&[], Vec::as_slice)
    }

    /// 结果栏当前缓存的总行数(测试断言用)。
    #[cfg(test)]
    fn line_count(&self) -> usize {
        self.chunks.iter().map(|c| c.spans.len()).sum()
    }
}

/// 块解决状态的指纹:结果内容仅由「取用顺序 + 覆写内容」决定,
/// 无需拼接行内容即可判断块是否变化。
fn state_fingerprint(state: &ChunkState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    state.order.len().hash(&mut hasher);
    for side in &state.order {
        matches!(side, crate::app::Side::Ours).hash(&mut hasher);
    }
    match &state.override_lines {
        None => false.hash(&mut hasher),
        Some(lines) => {
            true.hash(&mut hasher);
            lines.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// 按扩展名匹配语法定义;超大文件(三栏总行数超限)禁用。
fn find_syntax(merge: &FileMerge) -> Option<&'static SyntaxReference> {
    let total: usize = merge
        .chunks
        .iter()
        .map(|c| c.ours_lines().len() + c.theirs_lines().len() + c.base.len())
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
    light: bool,
) -> PaneSyntax {
    let set = syntax_set();
    let theme = syntect_theme(light);
    let highlighter = Highlighter::new(theme);
    let default_fg = theme.settings.foreground;
    let mut parse = ParseState::new(syntax);
    let mut hl = HighlightState::new(&highlighter, ScopeStack::new());
    let out = lines
        .map(|line| line_spans(&mut parse, &mut hl, &highlighter, default_fg, line, set))
        .collect();
    PaneSyntax { lines: out }
}

/// 用低层 API 高亮一行,推进解析 / 高亮状态,产出非默认前景的着色段。
///
/// newlines 版语法集要求输入行带换行符,产出区间时再剔除;
/// 等于主题默认前景的 token(普通标识符 / 标点)不产出着色段,
/// 交回终端默认前景,保持正文亮度;只有语义色 token(关键字 / 字符串等)上色。
fn line_spans(
    parse: &mut ParseState,
    hl: &mut HighlightState,
    highlighter: &Highlighter<'_>,
    default_fg: Option<SyntectColor>,
    line: &str,
    set: &SyntaxSet,
) -> Vec<(Color, Range<usize>)> {
    let with_nl = format!("{line}\n");
    let mut spans = Vec::new();
    let Ok(ops) = parse.parse_line(&with_nl, set) else {
        return spans;
    };
    let mut pos = 0usize;
    for (style, token) in HighlightIterator::new(hl, &ops, &with_nl, highlighter) {
        let end = pos + token.len();
        let clipped = end.min(line.len());
        if clipped > pos && Some(style.foreground) != default_fg {
            let fg = style.foreground;
            spans.push((super::theme::term_color(fg.r, fg.g, fg.b), pos..clipped));
        }
        pos = end;
    }
    spans
}

/// 计算一个块的词级强调;稳定块与超大块返回全空。
fn chunk_emphasis(chunk: &MergeChunk) -> ChunkEmphasis {
    let mut out = ChunkEmphasis {
        ours: vec![Emphasis::new(); chunk.ours_lines().len()],
        theirs: vec![Emphasis::new(); chunk.theirs_lines().len()],
    };
    let oversized = chunk.base.len() > EMPH_MAX_LINES
        || chunk.ours_lines().len() > EMPH_MAX_LINES
        || chunk.theirs_lines().len() > EMPH_MAX_LINES;
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
        side_emphasis(&chunk.base, chunk.ours_lines(), &mut out.ours);
    }
    if matches!(
        chunk.kind,
        ChunkKind::Theirs | ChunkKind::Agree | ChunkKind::Conflict
    ) {
        side_emphasis(&chunk.base, chunk.theirs_lines(), &mut out.theirs);
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
    for op in capture_diff_slices(Algorithm::Myers, chunk.ours_lines(), chunk.theirs_lines()) {
        if op.tag() != DiffTag::Replace {
            continue;
        }
        for (o, t) in op.old_range().zip(op.new_range()) {
            let (ours_line, theirs_line) = (&chunk.ours_lines()[o], &chunk.theirs_lines()[t]);
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
/// 两个视觉优化(与 delta 语义一致):
/// - 强调段之间仅隔纯空白时并入同一段,避免断裂的碎片感;
/// - 强调覆盖占比超过 [`EMPH_MAX_RATIO`] 时整行退化为纯色带,
///   几乎整行都变了时词级强调只剩噪点。
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
                // 相邻(间隙为空)或仅隔纯空白的区间并入同一段
                match ranges.last_mut() {
                    Some(last) if new[last.end..pos].chars().all(char::is_whitespace) => {
                        last.end = next;
                    }
                    _ => ranges.push(pos..next),
                }
                pos = next;
            }
        }
    }
    let covered: usize = ranges.iter().map(ExactSizeIterator::len).sum();
    if covered * 100 > new.len() * EMPH_MAX_RATIO {
        return Emphasis::new();
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

    /// 空白间隙桥接:相邻强调段之间只隔空格时并成一段连续强调
    #[test]
    fn whitespace_gaps_are_bridged() {
        let e = line_emphasis("prefix stays aa bb", "prefix stays xx yy");
        assert_eq!(e, vec![13..18]);
    }

    /// 强调覆盖占比超阈值的行退化为纯色带(无词级强调)
    #[test]
    fn mostly_changed_line_degrades_to_band() {
        let e = line_emphasis("old", "an entirely different line");
        assert!(e.is_empty());
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
        let hl = cache.get(0, &rs, 0, false);
        let ours = hl.ours.as_ref().unwrap();
        assert!(ours.lines.iter().any(|l| !l.is_empty()));

        let unknown = FileMerge::from_three_way("demo.unknownext".to_owned(), "a\n", "b\n", "a\n");
        let hl = cache.get(1, &unknown, 0, false);
        assert!(hl.ours.is_none());
        assert!(hl.result.is_none());
    }

    /// revision 变化时结果栏重算(行数随取用内容变化)
    #[test]
    fn result_pane_rehighlights_on_revision_change() {
        let mut merge =
            FileMerge::from_three_way("demo.rs".to_owned(), "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        let mut cache = HighlightCache::default();
        let before = cache.get(0, &merge, 0, false).result.as_ref().unwrap();
        assert_eq!(before.line_count(), 3);
        // 冲突两侧都取用 → 结果行数 3 → 4
        merge.apply(crate::app::Side::Ours);
        merge.apply(crate::app::Side::Theirs);
        let after = cache.get(0, &merge, 1, false).result.as_ref().unwrap();
        assert_eq!(after.line_count(), 4);
    }

    /// 增量重算与全量重建的着色结果完全一致(含取用 / 撤销 / 覆写序列)
    #[test]
    fn incremental_matches_full_recompute() {
        let base = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n";
        let ours = "fn a() { let x = 1; }\nfn b() {}\nfn c() { /* c */ }\nfn d() {}\n";
        let theirs = "fn a() { let y = \"s\"; }\nfn b() {}\nfn c() {}\nfn d() { unsafe {} }\n";
        let mut merge = FileMerge::from_three_way("demo.rs".to_owned(), base, ours, theirs);
        let mut cache = HighlightCache::default();
        let _ = cache.get(0, &merge, 0, false);

        // 依次:取本地、追加远端、撤销、覆写,每步都与全量重建对比
        let steps: [&dyn Fn(&mut FileMerge); 4] = [
            &|m| m.apply(crate::app::Side::Ours),
            &|m| m.apply(crate::app::Side::Theirs),
            &|m| m.undo(),
            &|m| m.set_override(vec!["fn a() { merged() }".to_owned()]),
        ];
        for (rev, step) in steps.iter().enumerate() {
            step(&mut merge);
            let incremental = cache.get(0, &merge, rev as u64 + 1, false);
            let mut fresh_cache = HighlightCache::default();
            let fresh = fresh_cache.get(0, &merge, 0, false);
            let (inc, full) = (
                incremental.result.as_ref().unwrap(),
                fresh.result.as_ref().unwrap(),
            );
            for idx in 0..merge.chunks.len() {
                for offset in 0..merge.current_content(idx).len() {
                    assert_eq!(
                        inc.spans(idx, offset),
                        full.spans(idx, offset),
                        "块 {idx} 行 {offset} 在第 {rev} 步后着色不一致"
                    );
                }
            }
        }
    }

    /// 增量重算只解析变化块(其余块靠指纹 + 状态收敛命中)
    #[test]
    fn incremental_update_skips_unchanged_chunks() {
        // 大稳定区 + 一个冲突 + 大稳定区:单键后只应重算冲突块
        let stable: String = (0..200).map(|i| format!("fn f{i}() {{}}\n")).collect();
        let base = format!("{stable}fn x() {{ old() }}\n{stable}");
        let ours = format!("{stable}fn x() {{ ours() }}\n{stable}");
        let theirs = format!("{stable}fn x() {{ theirs() }}\n{stable}");
        let mut merge = FileMerge::from_three_way("demo.rs".to_owned(), &base, &ours, &theirs);
        let total: usize = (0..merge.chunks.len())
            .map(|i| merge.current_content(i).len())
            .sum();

        let mut cache = HighlightCache::default();
        let first = cache.get(0, &merge, 0, false).result.as_ref().unwrap();
        assert_eq!(first.parsed_lines, total, "首次构建应全量解析");

        merge.apply(crate::app::Side::Ours);
        merge.ignore(crate::app::Side::Theirs);
        let second = cache.get(0, &merge, 1, false).result.as_ref().unwrap();
        assert!(
            second.parsed_lines <= 5,
            "增量重算应只解析变化块(实际解析 {} 行,全文 {total} 行)",
            second.parsed_lines
        );
    }

    /// 三栏总行数超限的文件禁用语法高亮
    #[test]
    fn oversized_file_disables_syntax() {
        let text: String = (0..4000).map(|i| format!("l{i}\n")).collect();
        let merge = FileMerge::from_three_way("big.rs".to_owned(), &text, &text, &text);
        let mut cache = HighlightCache::default();
        let hl = cache.get(0, &merge, 0, false);
        assert!(hl.ours.is_none());
        // 词级强调不受影响(稳定块本就为空)
        assert_eq!(hl.emphasis.len(), merge.chunks.len());
    }
}
