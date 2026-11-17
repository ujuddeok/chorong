//! Repository text search on ripgrep's libraries.
//!
//! Literal or regex patterns, case sensitivity, glob include/exclude (ripgrep semantics:
//! a `!` prefix excludes), `.gitignore` respected, hidden files skipped. Files are visited
//! in path order so results are deterministic. Hits are streamed to a callback; a result cap
//! and a cancellation token stop the walk early. Positions are UTF-8 byte offsets.

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkError, SinkMatch};
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use serde::Serialize;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Default cap on the number of matching lines returned.
pub const DEFAULT_MAX_RESULTS: usize = 5_000;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("invalid pattern: {0}")]
    Pattern(String),
    #[error("invalid glob: {0}")]
    Glob(String),
}

/// A cheap, clonable flag that stops a running search at the next file or line.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub pattern: String,
    /// Treat `pattern` as a regular expression; otherwise it is a literal string.
    pub regex: bool,
    pub case_sensitive: bool,
    /// ripgrep-style globs: `*.rs` includes only matching files, `!*.md` excludes.
    pub globs: Vec<String>,
    /// Stop after this many matching lines.
    pub max_results: usize,
}

impl SearchOptions {
    pub fn literal(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            regex: false,
            case_sensitive: false,
            globs: Vec::new(),
            max_results: DEFAULT_MAX_RESULTS,
        }
    }
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hit {
    /// Relative to the search root, `/`-separated.
    pub path: String,
    /// One-based.
    pub line: usize,
    /// Byte offset of the line's first byte in the file.
    pub line_start: usize,
    /// The line without its terminator.
    pub text: String,
    /// Byte ranges of every match, relative to `text`.
    pub ranges: Vec<Range<usize>>,
}

impl Hit {
    /// Byte offset in the file of the first match.
    pub fn first_match_byte(&self) -> usize {
        self.line_start + self.ranges.first().map_or(0, |r| r.start)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Matching lines delivered.
    pub hits: usize,
    /// The cap was reached; more matches may exist.
    pub truncated: bool,
    pub cancelled: bool,
}

/// Searches `root`, calling `on_hit` for each matching line in path order.
pub fn search(
    root: &Path,
    opts: &SearchOptions,
    cancel: &CancelToken,
    mut on_hit: impl FnMut(Hit),
) -> Result<Summary, SearchError> {
    let mut summary = Summary::default();
    if opts.pattern.is_empty() || opts.max_results == 0 {
        return Ok(summary);
    }
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!opts.case_sensitive)
        .fixed_strings(!opts.regex)
        .build(&opts.pattern)
        .map_err(|e| SearchError::Pattern(e.to_string()))?;

    let mut walk = WalkBuilder::new(root);
    walk.hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b));
    if !opts.globs.is_empty() {
        let mut ob = OverrideBuilder::new(root);
        for g in &opts.globs {
            ob.add(g).map_err(|e| SearchError::Glob(e.to_string()))?;
        }
        walk.overrides(ob.build().map_err(|e| SearchError::Glob(e.to_string()))?);
    }

    let mut searcher: Searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        // Offsets are into the file's own bytes, which is what the viewer opens: no BOM
        // stripping and no transcoding of UTF-16 files.
        .bom_sniffing(false)
        .line_number(true)
        .build();

    for entry in walk.build().flatten() {
        if cancel.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let mut sink = HitSink {
            matcher: &matcher,
            rel: &rel,
            cancel,
            on_hit: &mut on_hit,
            remaining: opts.max_results - summary.hits,
            delivered: 0,
            stopped_by_cap: false,
        };
        // Unreadable files are skipped, like ripgrep does.
        let _ = searcher.search_path(&matcher, path, &mut sink);
        summary.hits += sink.delivered;
        if sink.stopped_by_cap {
            summary.truncated = true;
            break;
        }
        if cancel.is_cancelled() {
            summary.cancelled = true;
            break;
        }
    }
    Ok(summary)
}

/// Collects every hit; convenient for the CLI and tests.
pub fn search_all(
    root: &Path,
    opts: &SearchOptions,
    cancel: &CancelToken,
) -> Result<(Vec<Hit>, Summary), SearchError> {
    let mut hits = Vec::new();
    let summary = search(root, opts, cancel, |h| hits.push(h))?;
    Ok((hits, summary))
}

struct HitSink<'a, F: FnMut(Hit)> {
    matcher: &'a grep_regex::RegexMatcher,
    rel: &'a str,
    cancel: &'a CancelToken,
    on_hit: &'a mut F,
    remaining: usize,
    delivered: usize,
    stopped_by_cap: bool,
}

#[derive(Debug)]
struct Never;
impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("search error")
    }
}
impl std::error::Error for Never {}
impl SinkError for Never {
    fn error_message<T: std::fmt::Display>(_: T) -> Self {
        Never
    }
}

impl<F: FnMut(Hit)> Sink for HitSink<'_, F> {
    type Error = Never;

    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Never> {
        if self.cancel.is_cancelled() {
            return Ok(false);
        }
        // A match block can hold several lines only in multi-line mode, which is off.
        let bytes = m.bytes();
        let Ok(line) = std::str::from_utf8(bytes) else {
            return Ok(true); // not UTF-8: Chorong only reads UTF-8 source
        };
        let text = line.trim_end_matches(['\n', '\r']);
        let mut ranges = Vec::new();
        let _ = self.matcher.find_iter(text.as_bytes(), |r| {
            // An empty match, or a byte-level one (`(?-u:.)`), can fall inside a character;
            // the viewer slices `text` by these ranges, so only whole characters count.
            if text.is_char_boundary(r.start()) && text.is_char_boundary(r.end()) {
                ranges.push(r.start()..r.end());
            }
            true
        });
        if ranges.is_empty() {
            return Ok(true);
        }
        if self.remaining == 0 {
            self.stopped_by_cap = true;
            return Ok(false);
        }
        (self.on_hit)(Hit {
            path: self.rel.to_string(),
            line: m.line_number().unwrap_or(0) as usize,
            line_start: m.absolute_byte_offset() as usize,
            text: text.to_string(),
            ranges,
        });
        self.delivered += 1;
        self.remaining -= 1;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cho-search-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join(".gitignore"), "/target\n").unwrap();
        fs::write(
            dir.join("src/a.rs"),
            "fn Alpha() {}\nlet x = alpha + ALPHA;\n\tbeta(); // héllo alpha\n",
        )
        .unwrap();
        fs::write(dir.join("src/b.md"), "alpha in docs\n").unwrap();
        fs::write(dir.join("target/out.rs"), "alpha ignored\n").unwrap();
        fs::write(dir.join("src/bin.dat"), b"alpha\0binary").unwrap();
        dir
    }

    fn run(dir: &Path, opts: &SearchOptions) -> Vec<Hit> {
        search_all(dir, opts, &CancelToken::new()).unwrap().0
    }

    #[test]
    fn literal_is_case_insensitive_by_default_and_reports_byte_ranges() {
        let dir = fixture("lit");
        let hits = run(&dir, &SearchOptions::literal("alpha"));
        let paths: Vec<_> = hits.iter().map(|h| (h.path.as_str(), h.line)).collect();
        assert_eq!(
            paths,
            [
                ("src/a.rs", 1),
                ("src/a.rs", 2),
                ("src/a.rs", 3),
                ("src/b.md", 1)
            ]
        );
        let l2 = &hits[1];
        assert_eq!(l2.text, "let x = alpha + ALPHA;");
        assert_eq!(l2.ranges, vec![8..13, 16..21]);
        assert_eq!(hits[0].line_start, 0);
        assert_eq!(l2.line_start, 14);
        // Byte offsets stay right after multibyte text and a tab.
        let l3 = &hits[2];
        assert_eq!(&l3.text[l3.ranges[0].clone()], "alpha");
        assert_eq!(l3.first_match_byte(), l3.line_start + l3.ranges[0].start);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn case_sensitive_and_ignored_and_binary_files() {
        let dir = fixture("case");
        let mut o = SearchOptions::literal("alpha");
        o.case_sensitive = true;
        let hits = run(&dir, &o);
        assert_eq!(hits.len(), 3, "Alpha and ALPHA do not match");
        assert!(hits.iter().all(|h| h.path != "target/out.rs"));
        assert!(hits.iter().all(|h| h.path != "src/bin.dat"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn literal_does_not_interpret_regex_syntax() {
        let dir = fixture("esc");
        fs::write(dir.join("src/c.txt"), "a.c abc\n").unwrap();
        assert_eq!(run(&dir, &SearchOptions::literal("a.c")).len(), 1);
        let mut o = SearchOptions::literal("a.c");
        o.regex = true;
        let hits = run(&dir, &o);
        assert_eq!(hits[0].ranges.len(), 2);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn globs_include_and_exclude() {
        let dir = fixture("glob");
        let mut o = SearchOptions::literal("alpha");
        o.globs = vec!["*.md".into()];
        assert_eq!(
            run(&dir, &o)
                .iter()
                .map(|h| &h.path[..])
                .collect::<Vec<_>>(),
            ["src/b.md"]
        );
        o.globs = vec!["!*.md".into()];
        assert!(run(&dir, &o).iter().all(|h| h.path == "src/a.rs"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn bad_regex_is_an_error() {
        let dir = fixture("bad");
        let mut o = SearchOptions::literal("(");
        o.regex = true;
        assert!(matches!(
            search_all(&dir, &o, &CancelToken::new()),
            Err(SearchError::Pattern(_))
        ));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn result_cap_truncates() {
        let dir = fixture("cap");
        let mut o = SearchOptions::literal("alpha");
        o.max_results = 2;
        let (hits, s) = search_all(&dir, &o, &CancelToken::new()).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(s.truncated);
        o.max_results = 4;
        let (hits, s) = search_all(&dir, &o, &CancelToken::new()).unwrap();
        assert_eq!(hits.len(), 4);
        assert!(!s.truncated);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn cancellation_stops_before_any_file() {
        let dir = fixture("cancel");
        let token = CancelToken::new();
        token.cancel();
        let (hits, s) = search_all(&dir, &SearchOptions::literal("alpha"), &token).unwrap();
        assert!(hits.is_empty() && s.cancelled);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn empty_pattern_finds_nothing() {
        let dir = fixture("empty");
        assert!(run(&dir, &SearchOptions::literal("")).is_empty());
        fs::remove_dir_all(dir).ok();
    }

    /// Found by the `search` fuzz target: a UTF-8 byte order mark was dropped before
    /// searching, so `line_start` of every later line was 3 bytes short of the file offset.
    #[test]
    fn byte_offsets_count_a_byte_order_mark() {
        let dir = fixture("bom");
        fs::write(dir.join("src/bom.txt"), "\u{feff}abc\nxabc\n").unwrap();
        let mut opts = SearchOptions::literal("abc");
        opts.globs = vec!["bom.txt".into()];
        let hits = run(&dir, &opts);
        let at: Vec<_> = hits
            .iter()
            .map(|h| (h.line, h.line_start, h.ranges[0].start, h.ranges[0].end))
            .collect();
        assert_eq!(at, [(1, 0, 3, 6), (2, 7, 1, 4)]);
        assert_eq!(hits[1].first_match_byte(), 8);
        // UTF-16 text is not transcoded: its bytes hold NULs, like any binary file.
        fs::write(dir.join("src/u16.txt"), b"\xff\xfea\0b\0c\0\n\0").unwrap();
        opts.globs = vec!["u16.txt".into()];
        assert!(run(&dir, &opts).is_empty());
    }

    /// Found by the `search` fuzz target: a regex that can match empty (`x*`) or a single byte
    /// (`(?-u:.)`) gave ranges inside multi-byte characters, which split them when shown.
    #[test]
    fn match_ranges_never_split_a_character() {
        let dir = fixture("chars");
        fs::write(dir.join("src/cjk.txt"), "  日本\t\n").unwrap();
        for pattern in ["x*", "(?-u:.)", "本"] {
            let mut opts = SearchOptions::literal(pattern);
            opts.regex = true;
            opts.globs = vec!["cjk.txt".into()];
            for h in run(&dir, &opts) {
                for r in &h.ranges {
                    assert!(
                        h.text.is_char_boundary(r.start) && h.text.is_char_boundary(r.end),
                        "{pattern}: {r:?}"
                    );
                }
            }
        }
    }
}
