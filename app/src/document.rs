//! Turning a source file into display lines with styled runs.
//!
//! Highlighting is computed once per file; rendering then only touches the
//! lines in view. Positions inside the buffer stay UTF-8 byte offsets; the
//! only conversion here is tab expansion for display.

use crate::fold::{Fold, folds_from_spans};
use cho_core::SourceBuffer;
use cho_syntax::{Analysis, HighlightSpan, Language, Symbol, SymbolKind, analyze, outline};
use cho_theme::{Rgba, Theme};
use gpui::{FontStyle, FontWeight, HighlightStyle, Hsla, SharedString};
use std::ops::Range;
use std::path::{Path, PathBuf};

pub const TAB_WIDTH: usize = 4;

/// JSON files up to this size get their folds before they show; a larger one shows at once
/// and folds in the background (`Document::folds_later`), since its extra parse takes about
/// 70 ms per MB in a release build.
pub const JSON_FOLDS_NOW_MAX: usize = 256 * 1024;

/// When a file's JSON folds are computed (see [`JSON_FOLDS_NOW_MAX`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonFolds {
    /// Not JSON, or `editor.json_folds` is off: none.
    None,
    /// While the file loads, before it shows.
    Now,
    /// After it shows, in the background.
    Later,
}

impl JsonFolds {
    /// The plan for a file of `len` bytes in `language`, with `editor.json_folds` `on` or off.
    pub fn plan(language: Option<Language>, len: usize, on: bool) -> Self {
        match language {
            Some(Language::Json) if on && len <= JSON_FOLDS_NOW_MAX => JsonFolds::Now,
            Some(Language::Json) if on => JsonFolds::Later,
            _ => JsonFolds::None,
        }
    }
}

pub fn hsla(c: Rgba) -> Hsla {
    gpui::rgba(c.to_u32()).into()
}

pub struct DisplayLine {
    pub text: SharedString,
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Byte range of the line in the file (without its newline).
    pub bytes: Range<usize>,
    /// Display offset of each byte of the line, for lines with tabs; `None` otherwise.
    pub tab_map: Option<Vec<usize>>,
    /// Call-name tokens on this line, in display offsets.
    pub calls: Vec<Range<usize>>,
    /// For each entry of `calls`, its index in `Analysis::calls` (to look up its kind).
    pub call_ids: Vec<usize>,
}

impl DisplayLine {
    /// Display offsets for a file byte range, clipped to this line.
    pub fn display_range(&self, range: &Range<usize>) -> Option<Range<usize>> {
        let start = range.start.max(self.bytes.start);
        let end = range.end.min(self.bytes.end);
        if start >= end {
            return None;
        }
        let at = |b: usize| match &self.tab_map {
            Some(map) => map[b - self.bytes.start],
            None => b - self.bytes.start,
        };
        Some(at(start)..at(end))
    }

    /// The file byte offset for a display offset in this line.
    pub fn byte_at(&self, display: usize) -> usize {
        self.bytes.start
            + crate::pointer::display_to_byte(self.tab_map.as_deref(), self.bytes.len(), display)
    }
}

/// One row of the outline panel, with lines resolved once at load time.
pub struct OutlineEntry {
    pub name: String,
    pub kind: SymbolKind,
    pub depth: usize,
    /// Byte offset of the name token (where a click lands).
    pub byte: usize,
    /// Lines the whole declaration spans.
    pub line_count: usize,
}

pub struct Document {
    pub path: PathBuf,
    /// Path shown in the UI: relative to the opened folder when there is one.
    pub display_path: String,
    /// Read from the git object store at this commit (abbreviated), not from the checkout.
    pub at_rev: Option<String>,
    pub language: Option<Language>,
    pub buffer: SourceBuffer,
    pub lines: Vec<DisplayLine>,
    pub symbols: Vec<Symbol>,
    pub outline: Vec<OutlineEntry>,
    pub folds: Vec<Fold>,
    /// The start lines of the folds that start collapsed: function and method bodies. The
    /// other folds (blocks, literals, imports) start open.
    pub folded_at_open: Vec<usize>,
    /// A large JSON file shown before its folds were found: the viewer finds them in the
    /// background ([`Document::json_folds`]) and adds them when they are ready.
    pub folds_later: bool,
    /// Identifier and call-name tokens and parse completeness from one Tree-sitter parse.
    pub analysis: Analysis,
}

impl Document {
    /// A rough count of the heap bytes this document holds (text, display lines and their
    /// highlights, symbols, outline, folds, tokens), to tell what keeping it costs.
    #[cfg(test)]
    pub fn approx_bytes(&self) -> usize {
        use std::mem::size_of;
        let lines: usize = self
            .lines
            .iter()
            .map(|l| {
                size_of::<DisplayLine>()
                    + l.text.len()
                    + l.highlights.len() * size_of::<(Range<usize>, HighlightStyle)>()
                    + l.tab_map
                        .as_ref()
                        .map_or(0, |m| m.len() * size_of::<usize>())
                    + l.calls.len() * size_of::<Range<usize>>()
                    + l.call_ids.len() * size_of::<usize>()
            })
            .sum();
        let symbols: usize = self
            .symbols
            .iter()
            .map(|s| {
                size_of::<Symbol>()
                    + s.name.len()
                    + s.scope.iter().map(|x| x.len() + 24).sum::<usize>()
            })
            .sum();
        self.buffer.text().len()
            + lines
            + symbols
            + self
                .outline
                .iter()
                .map(|o| size_of::<OutlineEntry>() + o.name.len())
                .sum::<usize>()
            + self.folds.len() * size_of::<Fold>()
            + (self.analysis.identifiers.len() + self.analysis.calls.len())
                * size_of::<Range<usize>>()
    }

    /// Loads `path` from disk. With a `root`, a path that resolves outside it (a symbolic link
    /// out of the repository) is refused; either way only regular files under the size limit
    /// are read (see `cho_core::fs`).
    pub fn load(
        path: PathBuf,
        root: Option<&Path>,
        display_path: String,
        theme: &Theme,
        json_folds: bool,
    ) -> anyhow::Result<Self> {
        let bytes = match root {
            Some(root) => cho_core::fs::read_source_in(root, &path)?,
            None => cho_core::fs::read_source(&path)?,
        };
        Self::from_bytes(path, display_path, bytes, None, theme, json_folds)
    }

    /// A document from `bytes`. `at_rev` marks a file read from the git object store at
    /// another commit (abbreviated id), shown read-only beside the checkout.
    pub fn from_bytes(
        path: PathBuf,
        display_path: String,
        bytes: Vec<u8>,
        at_rev: Option<String>,
        theme: &Theme,
        json_folds: bool,
    ) -> anyhow::Result<Self> {
        let buffer = SourceBuffer::from_bytes(bytes)?;
        let language = Language::from_path(&path);
        let spans = match language {
            // The thread's shared highlighter: queries compile once, not on every open.
            Some(l) => cho_syntax::highlight_shared(l, buffer.text()).unwrap_or_default(),
            None => Vec::new(),
        };
        let symbols = language
            .map(|l| outline(l, buffer.text()))
            .unwrap_or_default();
        // JSON's only analysis is its folds: skipped when off, or left for later when large.
        let json = JsonFolds::plan(language, buffer.len(), json_folds);
        let analysis = language
            .filter(|l| *l != Language::Json || json == JsonFolds::Now)
            .and_then(|l| analyze(l, buffer.text()))
            .unwrap_or_default();
        let lines = display_lines(&buffer, &spans, &analysis.calls, theme);
        let outline = outline_entries(&buffer, &symbols);
        // The signature is the line with the name, so attributes above it stay put.
        let bodies: Vec<(usize, usize)> = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
            .map(|s| {
                (
                    buffer.line_of(s.name_range.start),
                    buffer.line_of(s.range.end.saturating_sub(1)),
                )
            })
            .collect();
        // Every bracketed construct, Python block and import run folds too, but starts open.
        let regions = region_lines(&buffer, &analysis.folds);
        let folds = folds_from_spans(bodies.iter().copied().chain(regions));
        let folded_at_open = folds
            .iter()
            .filter(|f| bodies.iter().any(|(s, e)| *s == f.start && *e == f.end))
            .map(|f| f.start)
            .collect();
        Ok(Self {
            path,
            display_path,
            at_rev,
            language,
            buffer,
            lines,
            symbols,
            outline,
            folds,
            folded_at_open,
            folds_later: json == JsonFolds::Later,
            analysis,
        })
    }

    /// A JSON document's folds, found from its text (the background half of
    /// [`Document::folds_later`]). Empty for any other language.
    pub fn json_folds(&self) -> Vec<Fold> {
        if self.language != Some(Language::Json) {
            return Vec::new();
        }
        let regions = analyze(Language::Json, self.buffer.text())
            .map(|a| a.folds)
            .unwrap_or_default();
        folds_from_spans(region_lines(&self.buffer, &regions))
    }
}

/// Fold regions (byte ranges) as `(first line, last line)` pairs.
fn region_lines<'a>(
    buffer: &'a SourceBuffer,
    regions: &'a [Range<usize>],
) -> impl Iterator<Item = (usize, usize)> + 'a {
    regions.iter().map(|r| {
        (
            buffer.line_of(r.start),
            buffer.line_of(r.end.saturating_sub(1)),
        )
    })
}

fn outline_entries(buffer: &SourceBuffer, symbols: &[Symbol]) -> Vec<OutlineEntry> {
    symbols
        .iter()
        .map(|s| {
            let start = buffer.line_of(s.range.start);
            let end = buffer.line_of(s.range.end.saturating_sub(1));
            OutlineEntry {
                name: s.name.clone(),
                kind: s.kind,
                depth: s.scope.len(),
                byte: s.name_range.start,
                line_count: end.saturating_sub(start) + 1,
            }
        })
        .collect()
}

fn style_for(span: &HighlightSpan, theme: &Theme) -> Option<HighlightStyle> {
    key_style(span.theme_key(), theme)
}

/// The theme's style for a syntax theme key (`function.method`, …), as GPUI draws it.
pub fn key_style(key: &str, theme: &Theme) -> Option<HighlightStyle> {
    let s = theme.syntax_style(key)?;
    Some(HighlightStyle {
        color: s.color.map(hsla),
        font_style: s.font_style.map(|f| match f {
            cho_theme::FontStyle::Italic => FontStyle::Italic,
            cho_theme::FontStyle::Oblique => FontStyle::Oblique,
            cho_theme::FontStyle::Normal => FontStyle::Normal,
        }),
        font_weight: s.font_weight.map(|w| FontWeight(w as f32)),
        ..Default::default()
    })
}

/// Splits highlight spans by line and expands tabs, remapping ranges.
pub fn display_lines(
    buffer: &SourceBuffer,
    spans: &[HighlightSpan],
    calls: &[Range<usize>],
    theme: &Theme,
) -> Vec<DisplayLine> {
    let mut out = Vec::with_capacity(buffer.line_count());
    let mut next_span = 0;
    let mut next_call = 0;
    for line in 0..buffer.line_count() {
        let range = buffer.line_range(line).expect("line in range");
        let raw = &buffer.text()[range.clone()];
        let (text, map) = expand_tabs(raw);

        // Spans are sorted and disjoint: skip those ending before this line,
        // then take every span that overlaps it.
        while next_span < spans.len() && spans[next_span].range.end <= range.start {
            next_span += 1;
        }
        let mut highlights = Vec::new();
        let mut i = next_span;
        while i < spans.len() && spans[i].range.start < range.end {
            let span = &spans[i];
            let start = span.range.start.max(range.start) - range.start;
            let end = span.range.end.min(range.end) - range.start;
            if start < end
                && let Some(style) = style_for(span, theme)
            {
                highlights.push((map[start]..map[end], style));
            }
            i += 1;
        }
        while next_call < calls.len() && calls[next_call].end <= range.start {
            next_call += 1;
        }
        let (mut line_calls, mut call_ids) = (Vec::new(), Vec::new());
        for (id, c) in calls.iter().enumerate().skip(next_call) {
            if c.start >= range.end {
                break;
            }
            if c.start >= range.start && c.end <= range.end {
                line_calls.push(map[c.start - range.start]..map[c.end - range.start]);
                call_ids.push(id);
            }
        }
        out.push(DisplayLine {
            text: text.into(),
            highlights,
            bytes: range.clone(),
            tab_map: raw.contains('\t').then_some(map),
            calls: line_calls,
            call_ids,
        });
    }
    out
}

/// Expands tabs to the next tab stop. Returns the display text and a map from
/// every byte offset in `raw` (0..=len) to the matching display offset.
pub fn expand_tabs(raw: &str) -> (String, Vec<usize>) {
    if !raw.contains('\t') {
        return (raw.to_string(), (0..=raw.len()).collect());
    }
    let mut text = String::with_capacity(raw.len() + 8);
    let mut map = vec![0; raw.len() + 1];
    let mut column = 0;
    for (i, ch) in raw.char_indices() {
        for k in 0..ch.len_utf8() {
            map[i + k] = text.len();
        }
        if ch == '\t' {
            let width = TAB_WIDTH - column % TAB_WIDTH;
            text.extend(std::iter::repeat_n(' ', width));
            column += width;
        } else {
            text.push(ch);
            column += 1;
        }
    }
    map[raw.len()] = text.len();
    (text, map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_syntax::Highlighter;
    use cho_theme::Appearance;

    #[test]
    fn json_folds_wait_for_the_background_over_256_kb() {
        let json = Some(Language::Json);
        assert_eq!(
            JsonFolds::plan(json, JSON_FOLDS_NOW_MAX, true),
            JsonFolds::Now
        );
        assert_eq!(
            JsonFolds::plan(json, JSON_FOLDS_NOW_MAX + 1, true),
            JsonFolds::Later
        );
        assert_eq!(
            JsonFolds::plan(json, 10, false),
            JsonFolds::None,
            "the setting is off"
        );
        assert_eq!(
            JsonFolds::plan(Some(Language::Rust), 1 << 30, true),
            JsonFolds::None
        );
        // A real document on each side of the line: the same folds, now or later.
        let theme = Theme::bundled(Appearance::Dark);
        let entry = "  \"k\": [\n    1,\n    2\n  ],\n";
        let make = |n: usize| {
            let text = format!("{{\n{}  \"end\": 0\n}}\n", entry.repeat(n));
            let len = text.len();
            let doc = Document::from_bytes(
                PathBuf::from("a.json"),
                "a.json".into(),
                text.into_bytes(),
                None,
                &theme,
                true,
            )
            .unwrap();
            (len, doc)
        };
        let (len, small) = make(100);
        assert!(len <= JSON_FOLDS_NOW_MAX);
        assert!(!small.folds_later);
        assert_eq!(small.folds.len(), 101, "the root and each array");
        assert_eq!(small.json_folds(), small.folds);
        let (len, big) = make(JSON_FOLDS_NOW_MAX / entry.len() + 1);
        assert!(len > JSON_FOLDS_NOW_MAX);
        assert!(
            big.folds_later && big.folds.is_empty(),
            "shown without folds"
        );
        assert_eq!(big.json_folds().len(), JSON_FOLDS_NOW_MAX / entry.len() + 2);
        let off = Document::from_bytes(
            PathBuf::from("a.json"),
            "a.json".into(),
            b"{\n  \"a\": [\n    1,\n    2\n  ]\n}\n".to_vec(),
            None,
            &theme,
            false,
        )
        .unwrap();
        assert!(
            off.folds.is_empty() && !off.folds_later,
            "off: no folds at all"
        );
    }

    #[test]
    fn loads_outline_and_folds_without_a_window() {
        let dir_guard = tempfile::tempdir().unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.rs");
        std::fs::write(
            &path,
            "struct S;\nimpl S {\n    fn long(&self) {\n        let a = 1;\n        let b = 2;\n    }\n    fn short(&self) {}\n}\n",
        )
        .unwrap();
        let doc = Document::load(
            path,
            None,
            "a.rs".into(),
            &Theme::bundled(Appearance::Dark),
            true,
        )
        .unwrap();
        let long = doc.outline.iter().find(|e| e.name == "long").unwrap();
        assert_eq!((long.depth, long.byte, long.line_count), (1, 26, 4));
        // The `impl` body folds (open at first); the long method's body starts folded.
        let spans: Vec<_> = doc.folds.iter().map(|f| (f.start, f.end)).collect();
        assert_eq!(spans, [(1, 7), (2, 5)]);
        assert_eq!(doc.folded_at_open, [2]);
    }

    /// Extensions in `Language::ALL` order: the fuzz corpora's first byte picks the language.
    const EXTENSIONS: [&str; 7] = ["rs", "swift", "ts", "tsx", "js", "py", "java"];

    /// Opens `bytes` as `name` through the whole pipeline and checks that every derived
    /// table agrees with the buffer.
    fn check_document(name: &str, bytes: Vec<u8>, theme: &Theme) {
        let path = PathBuf::from(name);
        let Ok(doc) = Document::from_bytes(path, name.into(), bytes, None, theme, true) else {
            return; // binary, not UTF-8 or too large: refused, not mangled
        };
        let b = &doc.buffer;
        let text = b.text();
        assert_eq!(
            doc.lines.len(),
            b.line_count(),
            "{name}: one display line per line"
        );
        for (i, line) in doc.lines.iter().enumerate() {
            let range = b.line_range(i).unwrap();
            assert_eq!(line.bytes, range, "{name}: line {i} bytes");
            let raw = &text[range.clone()];
            assert_eq!(
                line.text.as_ref(),
                expand_tabs(raw).0.as_str(),
                "{name}: line {i} text"
            );
            let shown: &str = &line.text;
            let on_text = |r: &Range<usize>, what: &str| {
                assert!(
                    r.start < r.end && r.end <= shown.len(),
                    "{name}: line {i} {what} {r:?} of {}",
                    shown.len()
                );
                assert!(
                    shown.is_char_boundary(r.start) && shown.is_char_boundary(r.end),
                    "{name}: line {i} {what} {r:?} splits a character"
                );
            };
            let mut prev = 0;
            for (r, _) in &line.highlights {
                on_text(r, "highlight");
                assert!(r.start >= prev, "{name}: line {i} highlights overlap");
                prev = r.end;
            }
            assert_eq!(line.calls.len(), line.call_ids.len());
            for (r, &id) in line.calls.iter().zip(&line.call_ids) {
                on_text(r, "call");
                let c = &doc.analysis.calls[id];
                assert_eq!(line.display_range(c), Some(r.clone()), "{name}: call {id}");
            }
            // Every character boundary maps to a display offset and back.
            for (off, _) in raw.char_indices().chain([(raw.len(), ' ')]) {
                let at = range.start + off;
                let shown_at = match &line.tab_map {
                    Some(map) => map[off],
                    None => off,
                };
                assert!(shown_at <= shown.len());
                assert_eq!(line.byte_at(shown_at), at, "{name}: line {i} byte {at}");
            }
        }
        for f in &doc.folds {
            assert!(
                f.start < f.end && f.end < b.line_count(),
                "{name}: fold {f:?}"
            );
            let _ = f.hidden_lines();
        }
        assert_eq!(doc.outline.len(), doc.symbols.len());
        for (e, s) in doc.outline.iter().zip(&doc.symbols) {
            assert!(
                e.byte <= text.len() && text.is_char_boundary(e.byte),
                "{name}: {} at {}",
                e.name,
                e.byte
            );
            // Lines count from the declaration's start (attributes included), not its name.
            let first = b.line_of(s.range.start);
            assert!(
                e.line_count >= 1 && first + e.line_count <= b.line_count(),
                "{name}: {}",
                e.name
            );
            assert!(s.range.end <= text.len() && s.name_range.end <= text.len());
        }
        for r in doc.analysis.identifiers.iter().chain(&doc.analysis.calls) {
            assert!(
                r.end <= text.len() && text.is_char_boundary(r.start),
                "{name}: {r:?}"
            );
        }
    }

    /// xorshift64*, for generated documents.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n as u64) as usize
        }
    }

    /// The file-open pipeline (highlight, outline, analysis, display lines, folds) on
    /// generated files in every language: tabs, CRLF, wide and astral characters, broken
    /// syntax and nesting.
    #[test]
    fn opened_documents_agree_with_their_buffer() {
        const PIECES: [&str; 24] = [
            "fn f() {",
            "}",
            "\t",
            "\r\n",
            "\n",
            "class A {",
            "def g(x):\n    ",
            "func h() {",
            "é",
            "😀",
            "漢",
            "\"str\t\"",
            "// c\n",
            "/* c */",
            "(",
            ")",
            "x.y(z)",
            "let a = b(c);",
            "interface I { m(): void; }",
            "public void m() {}",
            "    ",
            "@A ",
            "import x",
            ";",
        ];
        let theme = Theme::bundled(Appearance::Dark);
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for n in 0..400 {
            let len = rng.below(120);
            let src: String = (0..len).map(|_| PIECES[rng.below(PIECES.len())]).collect();
            let ext = EXTENSIONS[n % EXTENSIONS.len()];
            check_document(&format!("gen{n}.{ext}"), src.into_bytes(), &theme);
        }
    }

    /// Replays fuzz corpora through the pipeline: `CHORONG_DOC_CORPUS=dir[:dir…] cargo test -p
    /// chorong-app corpus_documents -- --ignored`. Each file's first byte picks the language,
    /// as in the `highlight` and `outline` fuzz targets.
    #[test]
    #[ignore = "needs CHORONG_DOC_CORPUS"]
    fn corpus_documents() {
        let theme = Theme::bundled(Appearance::Dark);
        let dirs = std::env::var("CHORONG_DOC_CORPUS").expect("CHORONG_DOC_CORPUS");
        let mut seen = 0;
        for dir in dirs.split(':') {
            #[allow(clippy::disallowed_methods)] // a corpus folder the developer named
            let entries = std::fs::read_dir(dir).expect("corpus folder");
            for entry in entries.flatten() {
                #[allow(clippy::disallowed_methods)] // a corpus file the developer named
                let data = std::fs::read(entry.path()).unwrap();
                let Some((&sel, rest)) = data.split_first() else {
                    continue;
                };
                let ext = EXTENSIONS[sel as usize % EXTENSIONS.len()];
                let name = format!("{}.{ext}", entry.file_name().to_string_lossy());
                let bytes = String::from_utf8_lossy(rest).into_owned().into_bytes();
                check_document(&name, bytes, &theme);
                seen += 1;
            }
        }
        eprintln!("checked {seen} documents");
    }

    #[test]
    fn tabs_expand_to_stops_and_map_offsets() {
        let (text, map) = expand_tabs("a\tbc\td");
        assert_eq!(text, "a   bc  d");
        assert_eq!(map[2], 4); // 'b'
        assert_eq!(map[5], 8); // 'd'
        assert_eq!(map[6], 9);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn lines_map_file_bytes_to_display_and_back_through_tabs() {
        let theme = Theme::bundled(Appearance::Dark);
        let src = "fn a() {\n\tfoo(1);\n}\n";
        let buffer = SourceBuffer::new(src);
        let analysis = analyze(Language::Rust, src).unwrap();
        let lines = display_lines(&buffer, &[], &analysis.calls, &theme);
        let foo = src.find("foo").unwrap();
        // The tab shows as four spaces, so `foo` starts at display column 4.
        assert_eq!(lines[1].display_range(&(foo..foo + 3)), Some(4..7));
        assert_eq!(lines[1].calls, [4..7]);
        assert_eq!(lines[1].call_ids, [0]);
        assert_eq!(lines[1].byte_at(4), foo);
        assert_eq!(lines[1].byte_at(2), foo - 1, "inside the tab is the tab");
        assert_eq!(lines[0].display_range(&(foo..foo + 3)), None, "other line");
        assert_eq!(lines[0].byte_at(3), 3, "no tabs: identity");
    }

    /// Multibyte characters, tabs and CRLF line ends together: display text and offsets
    /// checked against literals, and every character boundary of every line maps to display
    /// and back to the same file byte.
    #[test]
    fn multibyte_tabs_and_crlf_map_exactly() {
        let theme = Theme::bundled(Appearance::Dark);
        let src = "\u{e9}\tx\r\n\u{1F600}\t\u{4E2D}\t\r\nplain\u{e9}\r\n";
        let buffer = SourceBuffer::new(src);
        let lines = display_lines(&buffer, &[], &[], &theme);
        // The `\r` is not part of a line; tab stops count characters (é is one column).
        assert_eq!(lines[0].text.as_ref(), "\u{e9}   x");
        assert_eq!(lines[1].text.as_ref(), "\u{1F600}   \u{4E2D}   ");
        assert_eq!(lines[2].text.as_ref(), "plain\u{e9}");
        let x = src.find('x').unwrap();
        assert_eq!(lines[0].display_range(&(x..x + 1)), Some(5..6));
        assert_eq!(lines[0].byte_at(5), x);
        assert_eq!(lines[0].byte_at(3), 2, "inside the tab is the tab");
        let zh = src.find('\u{4E2D}').unwrap();
        assert_eq!(lines[1].display_range(&(zh..zh + 3)), Some(7..10));
        assert_eq!(lines[1].byte_at(7), zh);
        for line in &lines {
            let raw = &src[line.bytes.clone()];
            assert!(!raw.contains('\r'), "{raw:?}");
            for (i, ch) in raw.char_indices() {
                let b = line.bytes.start + i;
                let d = line.display_range(&(b..b + ch.len_utf8())).unwrap();
                assert!(line.text.is_char_boundary(d.start), "{raw:?} byte {i}");
                assert_eq!(line.byte_at(d.start), b, "{raw:?} byte {i}");
            }
            let end = line
                .display_range(&(line.bytes.clone()))
                .map_or(0, |r| r.end);
            assert_eq!(end, line.text.len());
            assert_eq!(line.byte_at(end), line.bytes.end);
        }
    }

    #[test]
    fn spans_split_across_lines() {
        let theme = Theme::bundled(Appearance::Dark);
        let buffer = SourceBuffer::new("/* a\nb */ fn x() {}\n");
        let spans = Highlighter::new()
            .highlight(Language::Rust, buffer.text())
            .unwrap();
        let lines = display_lines(&buffer, &spans, &[], &theme);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].highlights[0].0, 0..4);
        assert_eq!(lines[1].highlights[0].0, 0..4);
        assert!(
            lines[1].highlights.len() > 1,
            "fn keyword is highlighted too"
        );
    }

    /// What keeping a parsed document costs, for typical and large files of this repository.
    /// Run with `--nocapture` to see the numbers.
    #[test]
    #[allow(clippy::disallowed_methods)] // reads this repository's own sources as fixtures
    fn approx_bytes_scales_with_the_file() {
        let theme = Theme::bundled(Appearance::Dark);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut sizes = Vec::new();
        for rel in [
            "crates/cho-core/src/buffer.rs",
            "app/src/view/compare.rs",
            "app/src/view/context_pane.rs",
        ] {
            let path = root.join(rel);
            let bytes = std::fs::read(&path).unwrap();
            let doc =
                Document::from_bytes(path, rel.into(), bytes.clone(), None, &theme, true).unwrap();
            println!(
                "{rel}: {} bytes of text, {} lines, ~{} bytes held",
                bytes.len(),
                doc.lines.len(),
                doc.approx_bytes()
            );
            sizes.push((bytes.len(), doc.approx_bytes()));
        }
        assert!(sizes.iter().all(|(text, held)| held > text));
    }
}
