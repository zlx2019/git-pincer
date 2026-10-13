//! Highlight computation and caching: delta-style word emphasis and syntect syntax highlighting.
//!
//! Word differences are computed in two stages: pair old and new lines by index
//! within Replace ranges, as delta does, then diff words to identify changed byte ranges.
//! Three-way mode compares each side with base; parsed `file` conflicts without a base
//! compare ours and theirs directly, highlighting words that differ from the other side.
//!
//! Syntax highlighting is computed per file, not per frame. Immutable ours / theirs
//! panes remain cached; result changes invalidate by `revision` and update incrementally
//! using cached parser states at chunk boundaries (see [`ResultSyntax`]). Only syntect
//! foreground colors are used, leaving backgrounds to change bands.

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

/// Skip word emphasis for chunks whose line count on either side exceeds this limit.
const EMPH_MAX_LINES: usize = 400;
/// Skip word diff for lines exceeding this byte limit.
const EMPH_MAX_LINE_BYTES: usize = 2000;
/// Maximum emphasized byte percentage before falling back to a plain whole-line band.
const EMPH_MAX_RATIO: usize = 70;
/// Disable syntect when the total line count across panes exceeds this limit (fancy-regex is slow).
const SYNTAX_MAX_LINES: usize = 10_000;

/// Process-wide syntax definitions, deserialized from the dump only once.
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Process-wide syntect themes, lazily built once for each dark / light variant.
fn syntect_theme(light: bool) -> &'static SyntectTheme {
    static DARK: OnceLock<SyntectTheme> = OnceLock::new();
    static LIGHT: OnceLock<SyntectTheme> = OnceLock::new();
    if light {
        LIGHT.get_or_init(|| maple(true))
    } else {
        DARK.get_or_init(|| maple(false))
    }
}

/// RGB tuple used by the syntax palette.
type Rgb = (u8, u8, u8);

/// Scope selectors mapped to (Maple Dark foreground, Maple Light foreground),
/// from tokenColors in maple-{dark,light}-color-theme.json.
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

/// Maple theme (https://github.com/subframe7536/vscode-theme-maple).
///
/// Maple is not bundled with syntect, so its core tokenColors scopes are ported
/// into a Theme built in code. Only foreground colors are used, without bold or italic,
/// leaving backgrounds to bands. The VSCode default foreground becomes terminal default.
fn maple(light: bool) -> SyntectTheme {
    let rgb = |(r, g, b): Rgb| SyntectColor { r, g, b, a: 0xff };
    let mut theme = SyntectTheme {
        name: Some(if light { "Maple Light" } else { "Maple Dark" }.to_owned()),
        ..SyntectTheme::default()
    };
    // editor.foreground (dark #cbd5e1 / light #475569); rendering skips tokens matching this default.
    theme.settings.foreground = Some(rgb(if light {
        (0x47, 0x55, 0x69)
    } else {
        (0xcb, 0xd5, 0xe1)
    }));
    for (scopes, dark, light_color) in TOKEN_COLORS {
        // Scope constants are fixed at compile time; skip invalid entries instead of panicking.
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

/// Changed byte ranges in one line, sorted, non-overlapping, and on character boundaries.
pub(crate) type Emphasis = Vec<Range<usize>>;

/// Word emphasis for a chunk, corresponding to each line in chunk.ours / chunk.theirs.
#[derive(Debug, Default)]
pub(crate) struct ChunkEmphasis {
    pub(crate) ours: Vec<Emphasis>,
    pub(crate) theirs: Vec<Emphasis>,
}

/// Pane syntax highlighting indexed by line number - 1, with (foreground, byte range) spans per line.
#[derive(Debug, Default)]
pub(crate) struct PaneSyntax {
    pub(crate) lines: Vec<Vec<(Color, Range<usize>)>>,
}

/// All highlighting information for one file.
#[derive(Debug)]
pub(crate) struct FileHighlight {
    /// Word emphasis per chunk, built once and valid for the file's lifetime
    pub(crate) emphasis: Vec<ChunkEmphasis>,
    /// Local pane syntax highlighting, built once for immutable content
    pub(crate) ours: Option<PaneSyntax>,
    /// Remote pane syntax highlighting, built once for immutable content
    pub(crate) theirs: Option<PaneSyntax>,
    /// Result pane syntax highlighting, incrementally updated by revision
    pub(crate) result: Option<ResultSyntax>,
    /// Matching syntax definition; None disables syntax highlighting for this file
    syntax: Option<&'static SyntaxReference>,
    /// Whether to use light syntax colors, retained for result updates
    light: bool,
    /// State revision represented by the result pane
    result_rev: u64,
}

impl FileHighlight {
    /// Build all highlighting information for a file.
    ///
    /// Syntax highlighting is independent per pane and CPU-bound (about 62 us/line).
    /// Scoped threads build panes concurrently, keeping first-frame latency near
    /// the slowest pane while word emphasis runs on the current thread.
    fn build(merge: &FileMerge, rev: u64, light: bool) -> Self {
        let syntax = find_syntax(merge);
        let (emphasis, ours, theirs, result) = std::thread::scope(|scope| {
            let ours = scope.spawn(|| {
                syntax.map(|s| {
                    highlight_pane(
                        &mut merge.chunks.iter().flat_map(|c| c.ours_lines().iter()),
                        s,
                        light,
                    )
                })
            });
            let theirs = scope.spawn(|| {
                syntax.map(|s| {
                    highlight_pane(
                        &mut merge.chunks.iter().flat_map(|c| c.theirs_lines().iter()),
                        s,
                        light,
                    )
                })
            });
            let result = scope.spawn(|| {
                syntax.map(|s| {
                    let mut result = ResultSyntax::default();
                    result.update(merge, s, light);
                    result
                })
            });
            let emphasis: Vec<ChunkEmphasis> = merge.chunks.iter().map(chunk_emphasis).collect();
            // Computation should not panic; gracefully disable syntax highlighting if it does.
            (
                emphasis,
                ours.join().unwrap_or_default(),
                theirs.join().unwrap_or_default(),
                result.join().unwrap_or_default(),
            )
        });
        Self {
            emphasis,
            ours,
            theirs,
            result,
            syntax,
            light,
            result_rev: rev,
        }
    }
}

/// Highlight information cached by file index.
#[derive(Debug, Default)]
pub(crate) struct HighlightCache {
    files: HashMap<usize, FileHighlight>,
}

impl HighlightCache {
    /// Get highlighting, building it on first access. Revision changes from taking,
    /// undoing, or editing incrementally update only changed result chunks.
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

/// Result syntax highlighting, caching parser boundary states and output per chunk.
///
/// Syntect parser state flows across lines, preventing independent chunk parsing.
/// Snapshot boundary states and recompute from the first changed chunk until a later
/// entry state matches the cache. This convergence reduces per-key cost from
/// O(whole file) to O(changed chunks + convergence tail).
#[derive(Debug, Default)]
pub(crate) struct ResultSyntax {
    /// Per-chunk caches corresponding one-to-one with chunks
    chunks: Vec<ChunkSyntax>,
    /// Lines actually parsed by the last update, exposing incremental work
    parsed_lines: usize,
}

/// Highlight cache for one result chunk.
#[derive(Debug)]
struct ChunkSyntax {
    /// Resolution fingerprint; content depends only on take order and overrides
    fingerprint: u64,
    /// Parser and highlighter entry states used for cache matching
    start: (ParseState, HighlightState),
    /// Exit states used to fast-forward through a cache hit
    end: (ParseState, HighlightState),
    /// Colored spans for each line in the chunk
    spans: Vec<Vec<(Color, Range<usize>)>>,
}

impl ResultSyntax {
    /// Update incrementally, reusing chunks with matching fingerprints and entry states.
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

    /// Get colored spans by chunk index and line offset within the chunk.
    pub(crate) fn spans(&self, chunk: usize, offset: usize) -> &[(Color, Range<usize>)] {
        self.chunks
            .get(chunk)
            .and_then(|c| c.spans.get(offset))
            .map_or(&[], Vec::as_slice)
    }

    /// Total cached result lines, used by test assertions.
    #[cfg(test)]
    fn line_count(&self) -> usize {
        self.chunks.iter().map(|c| c.spans.len()).sum()
    }
}

/// Fingerprint the take order and override content that determine a chunk's result,
/// detecting changes without concatenating its lines.
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

/// Match syntax by extension; disable it when total pane lines exceed the limit.
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
    // Plain Text has no color rules; using its theme foreground would dim the pane.
    // Disable highlighting so text uses the brighter terminal default foreground.
    (syntax.name != "Plain Text").then_some(syntax)
}

/// Highlight a pane in document order because syntect parser state flows across lines.
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

/// Highlight a line with the low-level API, advancing states and emitting non-default colors.
///
/// The newline-aware syntax set requires trailing newlines, removed from output ranges.
/// Tokens matching the theme's default foreground use the terminal default to preserve
/// brightness; only semantic tokens, such as keywords and strings, receive colors.
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

/// Compute word emphasis for a chunk; stable and oversized chunks return empty ranges.
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
        // With no base (parsed file conflicts or pure insertions), compare conflict sides directly.
        // Single-side insertions have no reference; their band is enough without word emphasis.
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

/// Pair lines against base, then compute inline differences on the changed side.
fn side_emphasis(base: &[String], side: &[String], out: &mut [Emphasis]) {
    for op in capture_diff_slices(Algorithm::Myers, base, side) {
        if op.tag() != DiffTag::Replace {
            continue;
        }
        // Pair Replace lines by index; extra lines receive no word emphasis.
        for (b, s) in op.old_range().zip(op.new_range()) {
            if let Some(slot) = out.get_mut(s) {
                *slot = line_emphasis(&base[b], &side[s]);
            }
        }
    }
}

/// For conflicts without base, compare ours and theirs and record each side's differences.
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

/// Return byte ranges in new that differ from old, merging adjacent word differences.
///
/// Two visual refinements, matching delta:
/// - Bridge whitespace-only gaps between emphasis ranges to avoid fragmentation.
/// - Use only the line band when emphasis exceeds [`EMPH_MAX_RATIO`], avoiding
///   noisy word highlighting when nearly the entire line changed.
///
/// Advance the cursor only for Equal and Insert, which belong to new, ensuring
/// ranges end on character boundaries and can be sliced safely during rendering.
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
                // Merge adjacent ranges or ranges separated only by whitespace.
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

    /// Helper for manually constructing chunks.
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

    /// Three-way mode precisely locates each side's changed words relative to base.
    #[test]
    fn emphasis_against_base() {
        let c = chunk(
            ChunkKind::Conflict,
            &["let a = 1;"],
            &["let a = 2;"],
            &["let b = 1;"],
        );
        let e = chunk_emphasis(&c);
        // Ours changes "2;" (word granularity includes adjacent punctuation); theirs changes "b".
        assert_eq!(e.ours[0], vec![8..10]);
        assert_eq!(e.theirs[0], vec![4..5]);
    }

    /// Conflicts without base fall back to comparing ours and theirs.
    #[test]
    fn empty_base_falls_back_to_cross_compare() {
        let c = chunk(ChunkKind::Conflict, &[], &["hello world"], &["hello rust"]);
        let e = chunk_emphasis(&c);
        assert_eq!(e.ours[0], vec![6..11]); // "world"
        assert_eq!(e.theirs[0], vec![6..10]); // "rust"
    }

    /// Bridge whitespace-only gaps into one continuous emphasis range.
    #[test]
    fn whitespace_gaps_are_bridged() {
        let e = line_emphasis("prefix stays aa bb", "prefix stays xx yy");
        assert_eq!(e, vec![13..18]);
    }

    /// Lines exceeding the emphasis ratio use only a band, without word emphasis.
    #[test]
    fn mostly_changed_line_degrades_to_band() {
        let e = line_emphasis("old", "an entirely different line");
        assert!(e.is_empty());
    }

    /// Oversized chunks fall back to empty emphasis.
    #[test]
    fn oversized_chunk_degrades_to_empty() {
        let lines: Vec<String> = (0..=EMPH_MAX_LINES).map(|i| format!("l{i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let c = chunk(ChunkKind::Conflict, &["a"], &refs, &["b"]);
        let e = chunk_emphasis(&c);
        assert!(e.ours.iter().all(Vec::is_empty));
        assert!(e.theirs.iter().all(Vec::is_empty));
    }

    /// .rs produces syntax colors; unknown extensions gracefully return None.
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

    /// Revision changes update result highlighting and line counts after taking content.
    #[test]
    fn result_pane_rehighlights_on_revision_change() {
        let mut merge =
            FileMerge::from_three_way("demo.rs".to_owned(), "a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
        let mut cache = HighlightCache::default();
        let before = cache.get(0, &merge, 0, false).result.as_ref().unwrap();
        assert_eq!(before.line_count(), 3);
        // Taking both conflict sides increases result lines from 3 to 4.
        merge.apply(crate::app::Side::Ours);
        merge.apply(crate::app::Side::Theirs);
        let after = cache.get(0, &merge, 1, false).result.as_ref().unwrap();
        assert_eq!(after.line_count(), 4);
    }

    /// Incremental colors match full rebuilds across take / undo / override sequences.
    #[test]
    fn incremental_matches_full_recompute() {
        let base = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n";
        let ours = "fn a() { let x = 1; }\nfn b() {}\nfn c() { /* c */ }\nfn d() {}\n";
        let theirs = "fn a() { let y = \"s\"; }\nfn b() {}\nfn c() {}\nfn d() { unsafe {} }\n";
        let mut merge = FileMerge::from_three_way("demo.rs".to_owned(), base, ours, theirs);
        let mut cache = HighlightCache::default();
        let _ = cache.get(0, &merge, 0, false);

        // Take local, append remote, undo, then override; compare each step with a full rebuild.
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
                        "highlighting differs for chunk {idx}, line {offset}, after step {rev}"
                    );
                }
            }
        }
    }

    /// Incremental updates parse only changed chunks, reusing fingerprints and converged states.
    #[test]
    fn incremental_update_skips_unchanged_chunks() {
        // A conflict between large stable regions should be the only chunk reparsed after a key press.
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
        assert_eq!(
            first.parsed_lines, total,
            "initial construction must parse all lines"
        );

        merge.apply(crate::app::Side::Ours);
        merge.ignore(crate::app::Side::Theirs);
        let second = cache.get(0, &merge, 1, false).result.as_ref().unwrap();
        assert!(
            second.parsed_lines <= 5,
            "incremental update must parse only changed chunks (parsed {} of {total} lines)",
            second.parsed_lines
        );
    }

    /// Files exceeding the total pane line limit disable syntax highlighting.
    #[test]
    fn oversized_file_disables_syntax() {
        let text: String = (0..4000).map(|i| format!("l{i}\n")).collect();
        let merge = FileMerge::from_three_way("big.rs".to_owned(), &text, &text, &text);
        let mut cache = HighlightCache::default();
        let hl = cache.get(0, &merge, 0, false);
        assert!(hl.ours.is_none());
        // Word emphasis is unaffected; stable chunks already have none.
        assert_eq!(hl.emphasis.len(), merge.chunks.len());
    }
}
