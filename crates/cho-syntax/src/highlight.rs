use crate::Language;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use thiserror::Error;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent};

/// Capture names Chorong asks the grammars for, paired with the Zed theme key
/// each one is drawn with. Grammars use nvim-style names
/// (`comment.documentation`, `keyword.function`); Zed themes use their own
/// (`comment.doc`, `keyword`). Theme lookup then falls back from a dotted key
/// to its parent, so `keyword.function` can still be styled on its own.
pub const CAPTURES: &[(&str, &str)] = &[
    ("attribute", "attribute"),
    ("boolean", "boolean"),
    ("character.special", "string.special"),
    ("comment", "comment"),
    ("comment.documentation", "comment.doc"),
    ("constant", "constant"),
    ("constant.builtin", "constant.builtin"),
    ("constant.macro", "constant"),
    ("constructor", "constructor"),
    ("embedded", "embedded"),
    ("escape", "string.escape"),
    ("function", "function"),
    ("function.builtin", "function.builtin"),
    ("function.call", "function"),
    ("function.macro", "function.macro"),
    ("function.method", "function.method"),
    ("keyword", "keyword"),
    ("keyword.conditional", "keyword"),
    ("keyword.coroutine", "keyword"),
    ("keyword.directive", "keyword"),
    ("keyword.exception", "keyword"),
    ("keyword.function", "keyword"),
    ("keyword.import", "keyword"),
    ("keyword.modifier", "keyword"),
    ("keyword.operator", "keyword"),
    ("keyword.repeat", "keyword"),
    ("keyword.return", "keyword"),
    ("keyword.type", "keyword"),
    ("label", "label"),
    ("module", "module"),
    ("number", "number"),
    ("number.float", "number"),
    ("operator", "operator"),
    ("property", "property"),
    ("punctuation", "punctuation"),
    ("punctuation.bracket", "punctuation.bracket"),
    ("punctuation.delimiter", "punctuation.delimiter"),
    ("punctuation.special", "punctuation.special"),
    ("string", "string"),
    ("string.escape", "string.escape"),
    ("string.regexp", "string.regex"),
    ("string.special", "string.special"),
    ("tag", "tag"),
    // Markdown (the grammar's own names; the keys are Zed's).
    ("text.emphasis", "emphasis"),
    ("text.literal", "text.literal"),
    ("text.reference", "link_text"),
    ("text.strong", "emphasis.strong"),
    ("text.title", "title"),
    ("text.uri", "link_uri"),
    ("type", "type"),
    ("type.builtin", "type.builtin"),
    ("variable", "variable"),
    ("variable.builtin", "variable.builtin"),
    ("variable.member", "property"),
    ("variable.parameter", "variable.parameter"),
];

/// Just the grammar-side capture names from [`CAPTURES`].
pub fn capture_names() -> Vec<&'static str> {
    CAPTURES.iter().map(|(capture, _)| *capture).collect()
}

#[derive(Debug, Error)]
pub enum HighlightError {
    #[error("failed to compile {language} highlight queries: {source}")]
    Query {
        language: &'static str,
        source: tree_sitter::QueryError,
    },
    #[error("highlighting failed: {0}")]
    Highlight(#[from] tree_sitter_highlight::Error),
    /// The syntax tree nests deeper than [`MAX_HIGHLIGHT_DEPTH`]: highlighting it would take
    /// minutes, so the file is shown as plain text.
    #[error("nested more than {MAX_HIGHLIGHT_DEPTH} levels deep; not highlighted")]
    TooDeep,
}

/// The deepest syntax tree [`Highlighter::highlight`] highlights. tree-sitter-highlight's
/// time grows with the square of the depth (100,000 nested brackets, or a 100,000-term sum,
/// take minutes); real code stays far below this, generated or hostile files may not.
pub const MAX_HIGHLIGHT_DEPTH: usize = 2000;

/// Does `source`'s syntax tree nest deeper than `limit`? Deep trees come from brackets but
/// also from long operator and call chains (`1+1+…`, `a.b().b()…`), so this parses rather
/// than guessing from the text: milliseconds for ordinary files, a second or two for
/// megabytes, against minutes for highlighting a deep tree.
fn too_deep(language: Language, source: &str, limit: usize) -> bool {
    // A tree is never deeper than the text is long.
    if source.len() <= limit {
        return false;
    }
    crate::outline::parse(language, source).is_some_and(|t| tree_depth(t.root_node()) > limit)
}

/// The languages of `source`'s fenced code blocks that can be highlighted. A language with any
/// block nested deeper than [`MAX_HIGHLIGHT_DEPTH`] is left out, so its blocks stay plain
/// rather than taking minutes.
fn fenced_languages(source: &str) -> HashSet<Language> {
    let Some(tree) = crate::outline::parse(Language::Markdown, source) else {
        return HashSet::new();
    };
    let (mut ok, mut deep) = (HashSet::new(), HashSet::new());
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "fenced_code_block" {
            let name = node
                .child_by_field_name("info_string")
                .and_then(|i| i.named_child(0).filter(|n| n.kind() == "language"))
                .and_then(|n| source.get(n.byte_range()))
                .and_then(Language::from_fence);
            let content = (0..node.named_child_count())
                .filter_map(|i| node.named_child(i as u32))
                .find(|c| c.kind() == "code_fence_content")
                .and_then(|c| source.get(c.byte_range()));
            if let (Some(l), Some(code)) = (name, content) {
                if too_deep(l, code, MAX_HIGHLIGHT_DEPTH) {
                    deep.insert(l);
                } else {
                    ok.insert(l);
                }
            }
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    &ok - &deep
}

/// The depth of the deepest node under `root` (the root is depth 1), walked without recursion.
pub(crate) fn tree_depth(root: tree_sitter::Node) -> usize {
    let mut cursor = root.walk();
    let (mut depth, mut max) = (1usize, 1usize);
    loop {
        if cursor.goto_first_child() {
            depth += 1;
            max = max.max(depth);
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return max;
            }
            depth -= 1;
        }
    }
}

/// A highlighted byte range. Spans never overlap and are sorted by start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    /// Index into [`CAPTURES`].
    pub capture: usize,
}

impl HighlightSpan {
    /// The grammar's capture name, e.g. `keyword.function`.
    pub fn capture_name(&self) -> &'static str {
        CAPTURES[self.capture].0
    }

    /// The Zed theme key to style this span with, e.g. `keyword`.
    pub fn theme_key(&self) -> &'static str {
        CAPTURES[self.capture].1
    }
}

/// Holds compiled highlight configurations. Compiling queries is slow, so
/// create one and reuse it.
#[derive(Default)]
pub struct Highlighter {
    configs: HashMap<Language, HighlightConfiguration>,
    /// Markdown's inline grammar, injected into its block grammar's `inline` nodes.
    markdown_inline: Option<HighlightConfiguration>,
    inner: tree_sitter_highlight::Highlighter,
}

impl Highlighter {
    pub fn new() -> Self {
        Self::default()
    }

    fn config(&mut self, language: Language) -> Result<&HighlightConfiguration, HighlightError> {
        if let std::collections::hash_map::Entry::Vacant(slot) = self.configs.entry(language) {
            let (highlights, injections, locals) = language.queries();
            let mut config = HighlightConfiguration::new(
                language.grammar(),
                language.name(),
                &highlights,
                injections,
                locals,
            )
            .map_err(|source| HighlightError::Query {
                language: language.name(),
                source,
            })?;
            config.configure(&capture_names());
            slot.insert(config);
        }
        if language == Language::Markdown && self.markdown_inline.is_none() {
            let mut config = HighlightConfiguration::new(
                tree_sitter_md::INLINE_LANGUAGE.into(),
                crate::language::MARKDOWN_INLINE,
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                "",
                "",
            )
            .map_err(|source| HighlightError::Query {
                language: language.name(),
                source,
            })?;
            config.configure(&capture_names());
            self.markdown_inline = Some(config);
        }
        Ok(&self.configs[&language])
    }

    /// Highlights `source`. Only the innermost capture is kept for each byte,
    /// so the result is a flat, non-overlapping list.
    pub fn highlight(
        &mut self,
        language: Language,
        source: &str,
    ) -> Result<Vec<HighlightSpan>, HighlightError> {
        self.config(language)?;
        if too_deep(language, source, MAX_HIGHLIGHT_DEPTH) {
            return Err(HighlightError::TooDeep);
        }
        // Fenced code is highlighted by its language: compile those first, since the
        // injection callback can only borrow configurations that already exist.
        let fenced = if language == Language::Markdown {
            fenced_languages(source)
        } else {
            HashSet::new()
        };
        for &l in &fenced {
            self.config(l)?;
        }
        let configs = &self.configs;
        let config = &configs[&language];
        let inline = self.markdown_inline.as_ref();
        let events = self
            .inner
            .highlight(config, source.as_bytes(), None, None, |name| {
                if name == crate::language::MARKDOWN_INLINE {
                    return inline;
                }
                Language::from_fence(name)
                    .filter(|l| fenced.contains(l))
                    .and_then(|l| configs.get(&l))
            })?;
        let mut spans = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for event in events {
            match event? {
                HighlightEvent::HighlightStart(h) => stack.push(h.0),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    if let Some(&capture) = stack.last() {
                        match spans.last_mut() {
                            Some(HighlightSpan { range, capture: c })
                                if *c == capture && range.end == start =>
                            {
                                range.end = end
                            }
                            _ => spans.push(HighlightSpan {
                                range: start..end,
                                capture,
                            }),
                        }
                    }
                }
            }
        }
        Ok(spans)
    }
}

thread_local! {
    /// One compiled highlighter per thread, reused by [`highlight_shared`].
    static SHARED: std::cell::RefCell<Highlighter> = std::cell::RefCell::new(Highlighter::new());
}

/// Highlights `source` with this thread's shared [`Highlighter`], so each language's queries
/// are compiled once per thread instead of once per call. Use this for anything that
/// highlights repeatedly (opening files, context previews).
pub fn highlight_shared(
    language: Language,
    source: &str,
) -> Result<Vec<HighlightSpan>, HighlightError> {
    SHARED.with(|h| h.borrow_mut().highlight(language, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by the large-input fuzz runs: 100,000 nested brackets took over two minutes
    /// to highlight. Past the depth limit the file is plain text, at once.
    #[test]
    fn deep_nesting_is_not_highlighted() {
        let deep = format!("fn f() {}{}", "{".repeat(100_000), "}".repeat(100_000));
        let started = std::time::Instant::now();
        let r = Highlighter::new().highlight(Language::Rust, &deep);
        assert!(matches!(r, Err(HighlightError::TooDeep)), "{r:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        let chain = format!("x={}1;", "1+".repeat(50_000));
        let r = Highlighter::new().highlight(Language::JavaScript, &chain);
        assert!(matches!(r, Err(HighlightError::TooDeep)), "{r:?}");
        // Many brackets that don't nest in the tree (a string) are still highlighted.
        let s = format!("fn f() {{ let s = \"{}\"; }}", "(".repeat(5000));
        assert!(Highlighter::new().highlight(Language::Rust, &s).is_ok());
        let tree = crate::outline::parse(Language::Rust, "fn f() { g(1); }").unwrap();
        assert!(tree_depth(tree.root_node()) > 3);
    }

    fn capture_at(spans: &[HighlightSpan], source: &str, needle: &str) -> Option<&'static str> {
        let at = source.find(needle).expect("needle present");
        spans
            .iter()
            .find(|s| s.range.contains(&at))
            .map(HighlightSpan::theme_key)
    }

    #[test]
    fn every_language_compiles_and_highlights() {
        let samples = [
            (
                Language::Rust,
                "// hi\nfn main() { let x = 1; }\n",
                "fn",
                "keyword",
            ),
            (
                Language::Swift,
                "func f() -> Int { return 1 }\n",
                "func",
                "keyword",
            ),
            (
                Language::TypeScript,
                "interface A { x: number }\n",
                "interface",
                "keyword",
            ),
            (Language::Tsx, "const a = <div />;\n", "const", "keyword"),
            (
                Language::JavaScript,
                "function f() { return 'x' }\n",
                "function",
                "keyword",
            ),
            (
                Language::Python,
                "def f():\n    return 1\n",
                "def",
                "keyword",
            ),
            (
                Language::Java,
                "public class A { void f() {} }\n",
                "class",
                "keyword",
            ),
        ];
        let mut h = Highlighter::new();
        for (language, source, needle, expected) in samples {
            let spans = h.highlight(language, source).unwrap();
            assert_eq!(
                capture_at(&spans, source, needle),
                Some(expected),
                "{language:?}"
            );
        }
    }

    /// The highlight-only formats colour keys, strings, numbers, literals and Markdown markup.
    #[test]
    fn data_format_captures() {
        /// A language, a source, and (needle, expected theme key) pairs.
        type Case = (
            Language,
            &'static str,
            &'static [(&'static str, &'static str)],
        );
        let cases: [Case; 4] = [
            (
                Language::Json,
                "{\"name\": \"cho\", \"n\": 12, \"ok\": true, \"x\": null}\n",
                &[
                    ("name", "property"),
                    ("\"cho", "string"),
                    ("12", "number"),
                    ("true", "constant.builtin"),
                    ("null", "constant.builtin"),
                ],
            ),
            (
                Language::Toml,
                "# c\n[package]\nname = \"cho\"\nversion.workspace = true\nn = 3\n",
                &[
                    ("# c", "comment"),
                    ("package", "type"),
                    ("name", "property"),
                    ("\"cho", "string"),
                    ("version", "property"),
                    ("true", "boolean"),
                    ("3", "number"),
                ],
            ),
            (
                Language::Xml,
                "<?xml version=\"1.0\"?>\n<!-- c -->\n<root id=\"a\">text</root>\n",
                &[
                    ("root", "tag"),
                    ("id", "property"),
                    ("a\">", "string"),
                    ("<!--", "comment"),
                    ("1.0", "number"),
                ],
            ),
            (
                Language::Markdown,
                "# Head\n\nSome *em*, **bold**, `code` and [link](http://x.y).\n\n- item\n\n```\nraw\n```\n",
                &[
                    ("#", "punctuation.special"),
                    ("Head", "title"),
                    ("em*", "emphasis"),
                    ("bold", "emphasis.strong"),
                    ("code", "text.literal"),
                    ("link]", "link_text"),
                    ("http", "link_uri"),
                    ("- ", "punctuation.special"),
                    ("raw", "text.literal"),
                ],
            ),
        ];
        let mut h = Highlighter::new();
        for (language, source, expected) in cases {
            let spans = h.highlight(language, source).unwrap();
            for (needle, key) in expected {
                assert_eq!(
                    capture_at(&spans, source, needle),
                    Some(*key),
                    "{language:?} {needle}"
                );
            }
        }
    }

    /// Fenced code is highlighted by its language; an unknown language stays one colour, and
    /// a block nested too deep to highlight is left plain.
    #[test]
    fn markdown_fenced_code() {
        let source = "```rust\nfn main() { let x = 1; }\n```\n\n- list\n\n  ```json\n  {\"k\": 2}\n  ```\n\n```bash\necho hi\n```\n";
        let spans = Highlighter::new()
            .highlight(Language::Markdown, source)
            .unwrap();
        for (needle, key) in [
            ("rust", "label"),
            ("fn", "keyword"),
            ("main", "function"),
            ("1;", "constant.builtin"),
            ("\"k\"", "property"),
            ("2}", "number"),
            ("echo", "text.literal"),
        ] {
            assert_eq!(capture_at(&spans, source, needle), Some(key), "{needle}");
        }
        let deep = format!(
            "```rust\nfn f() {}{}\n```\n\n```json\n[1]\n```\n",
            "{".repeat(MAX_HIGHLIGHT_DEPTH + 10),
            "}".repeat(MAX_HIGHLIGHT_DEPTH + 10)
        );
        let started = std::time::Instant::now();
        let spans = Highlighter::new()
            .highlight(Language::Markdown, &deep)
            .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert_eq!(capture_at(&spans, &deep, "fn"), None, "too deep: plain");
        assert_eq!(capture_at(&spans, &deep, "1]"), Some("number"));
    }

    /// Data formats are highlight-only: no declarations, calls or references (JSON's objects
    /// and arrays fold, nothing more: see `analysis::json_folds`).
    #[test]
    fn data_formats_have_no_structure() {
        for (language, source) in [
            (Language::Json, "{\"f\": {\"g\": [1]}}"),
            (Language::Toml, "[a]\nf = { g = 1 }\n"),
            (Language::Xml, "<f><g a=\"1\"/></f>"),
            (Language::Markdown, "# f\n\n- g\n"),
        ] {
            assert!(crate::outline(language, source).is_empty());
            let analysis = crate::analyze(language, source).unwrap();
            if language != Language::Json {
                assert_eq!(analysis, crate::Analysis::default());
            }
            assert_eq!(
                crate::Analysis {
                    folds: Vec::new(),
                    ..analysis
                },
                crate::Analysis::default()
            );
            assert_eq!(crate::classify_references(language, source), Some(vec![]));
        }
    }

    #[test]
    fn java_captures() {
        let source = "@interface Ann {}\n/** doc */\nclass A<T> { static final int MAX = 1; enum E { RED } String s = \"x\\n\"; void run() { list.forEach(this::go); } }\n";
        let spans = Highlighter::new()
            .highlight(Language::Java, source)
            .unwrap();
        for (needle, key) in [
            ("@interface", "keyword"),
            ("Ann", "type"),
            ("A<T>", "type"),
            ("String s", "type"),
            ("/** doc", "comment"),
            ("MAX", "constant"),
            ("RED", "constant"),
            ("\"x", "string"),
            ("\\n", "string.escape"),
            ("run", "function.method"),
            ("go)", "function.method"),
            ("int MAX", "type.builtin"),
        ] {
            assert_eq!(capture_at(&spans, source, needle), Some(key), "{needle}");
        }
    }

    #[test]
    fn shared_highlighter_matches_a_fresh_one() {
        let source = "pub fn f(a: Foo) -> Option<usize> {";
        let fresh = Highlighter::new()
            .highlight(Language::Rust, source)
            .unwrap();
        assert_eq!(highlight_shared(Language::Rust, source).unwrap(), fresh);
        assert_eq!(highlight_shared(Language::Rust, source).unwrap(), fresh);
    }

    #[test]
    fn a_signature_gets_keyword_function_and_type_spans() {
        // Context previews highlight a definition's line with the lines after it as context.
        let source = "pub fn utf16_to_byte(&self, pos: LineCol) -> Option<usize> {\n    None\n}\n";
        let spans = highlight_shared(Language::Rust, source).unwrap();
        let key = |spans: &[HighlightSpan], src: &str, needle: &str| {
            let at = src.find(needle).unwrap();
            spans
                .iter()
                .find(|s| s.range.contains(&at))
                .map(|s| s.theme_key())
        };
        assert_eq!(key(&spans, source, "fn "), Some("keyword"));
        assert_eq!(key(&spans, source, "utf16_to_byte"), Some("function"));
        assert_eq!(key(&spans, source, "LineCol"), Some("type"));
        // The line alone parses as an error and its name loses the colour: why the context
        // pane reads on past the line for definitions.
        let line = source.lines().next().unwrap();
        let alone = highlight_shared(Language::Rust, line).unwrap();
        assert_eq!(key(&alone, line, "utf16_to_byte"), None);
        assert_eq!(key(&alone, line, "LineCol"), Some("type"));
    }

    /// `cargo test -p cho-syntax --release -- --ignored --nocapture highlight_cost`
    #[test]
    #[ignore]
    fn highlight_cost() {
        let file = include_str!("../../cho-core/src/buffer.rs");
        let line = "pub fn utf16_to_byte(&self, pos: LineCol) -> Option<usize> {";
        let t = std::time::Instant::now();
        let mut h = Highlighter::new();
        h.highlight(Language::Rust, line).unwrap();
        println!("compile + 1 line: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        for _ in 0..300 {
            h.highlight(Language::Rust, line).unwrap();
        }
        println!("300 lines, compiled: {:?}", t.elapsed());
        let at = file.find("    pub fn utf16_to_byte").unwrap();
        let window: String = file[at..].lines().take(30).collect::<Vec<_>>().join("\n");
        let t = std::time::Instant::now();
        for _ in 0..100 {
            h.highlight(Language::Rust, window.trim_start()).unwrap();
        }
        println!(
            "definition window (30 lines), each: {:?}",
            t.elapsed() / 100
        );
        let t = std::time::Instant::now();
        h.highlight(Language::Rust, file).unwrap();
        println!("whole file ({} bytes): {:?}", file.len(), t.elapsed());
    }

    #[test]
    fn spans_are_sorted_disjoint_and_on_char_boundaries() {
        let source = "/// é 😀\nfn f(a: &str) -> String { format!(\"{a}\") }\n";
        let spans = Highlighter::new()
            .highlight(Language::Rust, source)
            .unwrap();
        assert!(!spans.is_empty());
        for pair in spans.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start);
        }
        for s in &spans {
            assert!(source.is_char_boundary(s.range.start) && source.is_char_boundary(s.range.end));
        }
        assert_eq!(capture_at(&spans, source, "/// é"), Some("comment.doc"));
    }
}
