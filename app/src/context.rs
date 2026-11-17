//! What the context pane shows for a symbol under the caret, answered from the index.
//!
//! Everything here comes from SCIP: nothing is matched by name. Gathering reads files for
//! previews, so callers run it off the UI thread.

use crate::features::Probe;
use cho_core::SourceBuffer;
use cho_semantic::{Location, Semantic, SymbolId};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

/// Most reference rows kept; the rest are counted, not listed.
pub const MAX_REFERENCES: usize = 300;
const PREVIEW_CHARS: usize = 110;
/// Lines (and at most bytes) after a definition that its highlighting reads as context.
const CONTEXT_LINES: usize = 30;
const CONTEXT_BYTES: usize = 4096;

/// One place in a file with its source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: String,
    /// One-based.
    pub line: usize,
    /// Byte offset in the file, where a click lands.
    pub byte: usize,
    /// The source line, trimmed.
    pub preview: String,
    /// The occurrence's own bytes within `preview` (from the index's range, not a name
    /// match), drawn in the symbol's kind colour. None when the preview cut it off.
    pub mark: Option<Range<usize>>,
    /// Syntax spans over `preview` as theme keys (`keyword`, `function`, …), sorted and
    /// disjoint. Highlighted here, off the UI thread; the view only paints them.
    pub spans: Vec<(Range<usize>, &'static str)>,
    /// `mapped (unverified): …` when the place belongs to a symbol joined to the asked one
    /// by a dist-to-source mapping, not by the index.
    pub mapped: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRefs {
    pub path: String,
    pub places: Vec<Place>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolContext {
    pub name: String,
    pub kind: String,
    /// The occurrence under the caret.
    pub range: Range<usize>,
    pub definitions: Vec<Place>,
    pub references: Vec<FileRefs>,
    pub reference_count: usize,
    /// References beyond [`MAX_REFERENCES`] that are counted but not listed.
    pub omitted: usize,
}

/// The index's answer for one caret position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The index has no symbol at this position.
    NoSymbol,
    Symbol(Box<SymbolContext>),
}

/// Asks the index about the byte `byte` of `path` (repository-relative, `/`-separated).
///
/// `references` is `Some` while `context.references` is on: the lookup and previews of the
/// reference list run (and are timed with it) only then.
pub fn gather(
    sem: &Semantic,
    root: &Path,
    path: &str,
    byte: usize,
    references: Option<&Probe>,
) -> Answer {
    let Some(at) = sem.symbol_at(path, byte) else {
        return Answer::NoSymbol;
    };
    let (name, kind) = match sem.info(&at.id) {
        Some(info) => (info.display_name.clone(), info.kind.clone()),
        None => (display_from_symbol(&at.id), String::new()),
    };
    let mut previews = Previews::new(root);
    let definitions: Vec<Place> = sem
        .definitions(&at.id)
        .iter()
        .map(|l| previews.place(l, true))
        .collect();
    // `references` lists definitions first; the pane lists them separately.
    let _t = references.map(Probe::start);
    let refs: Vec<Location> = if references.is_some() {
        sem.references(&at.id)
            .into_iter()
            .filter(|l| !l.is_definition)
            .collect()
    } else {
        Vec::new()
    };
    let reference_count = refs.len();
    let mut files: Vec<FileRefs> = Vec::new();
    for l in refs.iter().take(MAX_REFERENCES) {
        let place = previews.place(l, false);
        match files.last_mut() {
            Some(f) if f.path == place.path => f.places.push(place),
            _ => files.push(FileRefs {
                path: place.path.clone(),
                places: vec![place],
            }),
        }
    }
    Answer::Symbol(Box::new(SymbolContext {
        name,
        kind,
        range: at.range,
        definitions,
        references: files,
        reference_count,
        omitted: reference_count.saturating_sub(MAX_REFERENCES),
    }))
}

/// Syntax spans for one reference preview line of the file at `path`, highlighted alone
/// (statements parse well enough out of context). About 40 µs once the thread's highlighter
/// is compiled; run it off the UI thread.
pub fn highlight_line(path: &str, line: &str) -> Vec<(Range<usize>, &'static str)> {
    cho_syntax::Language::from_path(Path::new(path))
        .and_then(|lang| cho_syntax::highlight_shared(lang, line).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.range.clone(), s.theme_key()))
        .collect()
}

/// A readable name for a symbol the index has no display name for: the last descriptor.
fn display_from_symbol(id: &SymbolId) -> String {
    let s = id.symbol.trim_end_matches(['.', '#', '/', ')', '(']);
    s.rsplit(['.', '#', '/', ' '])
        .next()
        .unwrap_or(s)
        .to_string()
}

/// Reads each file once to quote its lines, and highlights each quoted line.
struct Previews<'a> {
    root: &'a Path,
    files: HashMap<String, Option<SourceBuffer>>,
}

impl<'a> Previews<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            files: HashMap::new(),
        }
    }

    /// The place `l`, its preview trimmed. A `definition` is also highlighted, with the lines
    /// after it as context (a lone `fn f() -> T {` parses as an error, so its name would lose
    /// its colour). Reference lines are highlighted later, only once on screen
    /// ([`highlight_line`]), since there can be hundreds.
    fn place(&mut self, l: &Location, definition: bool) -> Place {
        let buf = self.files.entry(l.path.clone()).or_insert_with(|| {
            cho_core::fs::read_source_in(self.root, Path::new(&l.path))
                .ok()
                .and_then(|b| SourceBuffer::from_bytes(b).ok())
        });
        let language = cho_syntax::Language::from_path(Path::new(&l.path));
        let line = buf.as_ref().and_then(|b| {
            let start = b.line_range(l.line - 1)?.start;
            Some((b, start, b.line_text(l.line - 1)?))
        });
        let Some((b, start, t)) = line else {
            return Place {
                path: l.path.clone(),
                line: l.line,
                byte: l.range.start,
                preview: String::new(),
                mark: None,
                spans: Vec::new(),
                mapped: l.mapped.as_ref().map(|m| m.mapping.label()),
            };
        };
        let preview: String = t.trim().chars().take(PREVIEW_CHARS).collect();
        let lead = start + (t.len() - t.trim_start().len());
        let mark = l
            .range
            .start
            .checked_sub(lead)
            .zip(l.range.end.checked_sub(lead))
            .map(|(a, b)| a..b)
            .filter(|r| {
                r.start < r.end
                    && r.end <= preview.len()
                    && preview.is_char_boundary(r.start)
                    && preview.is_char_boundary(r.end)
            });
        // Once the thread's highlighter is compiled, a definition's window costs a few hundred
        // microseconds (measured in cho-syntax's `highlight_cost`).
        if !definition {
            return Place {
                path: l.path.clone(),
                line: l.line,
                byte: l.range.start,
                preview,
                mark,
                spans: Vec::new(),
                mapped: l.mapped.as_ref().map(|m| m.mapping.label()),
            };
        }
        let source = {
            let text = b.text();
            let mut end = b
                .line_range(l.line - 1 + CONTEXT_LINES)
                .map_or(text.len(), |r| r.end)
                .min(lead + CONTEXT_BYTES)
                .min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[lead..end.max(lead + preview.len())]
        };
        let spans = language
            .and_then(|lang| cho_syntax::highlight_shared(lang, source).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.range.start < preview.len())
            .map(|s| (s.range.start..s.range.end.min(preview.len()), s.theme_key()))
            .filter(|(r, _)| preview.is_char_boundary(r.end))
            .collect();
        Place {
            path: l.path.clone(),
            line: l.line,
            byte: l.range.start,
            preview,
            mark,
            spans,
            mapped: l.mapped.as_ref().map(|m| m.mapping.label()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_semantic::IndexStatus;
    use cho_semantic::index::{Document, Encoding, Occurrence, RawRange, ScipIndex, SymbolInfo};

    const HELPER: &str = "pkg helper().";

    fn occ(line: u32, col: u32, end: u32, def: bool) -> Occurrence {
        Occurrence {
            range: RawRange {
                start_line: line,
                start_col: col,
                end_line: line,
                end_col: end,
            },
            symbol: HELPER.into(),
            roles: def as i32,
        }
    }

    fn probe() -> Probe {
        crate::features::Features::with_store(cho_settings::SettingsStore::in_memory())
            .probe(crate::features::Feature::References)
    }

    #[test]
    fn with_references_off_none_are_looked_up() {
        let (_guard, dir, sem) = fixture();
        let Answer::Symbol(c) = gather(&sem, &dir, "src/a.rs", 26, None) else {
            panic!("a symbol");
        };
        assert!(!c.definitions.is_empty(), "definitions still answer");
        assert!(c.references.is_empty());
        assert_eq!(c.reference_count, 0);
    }

    /// The committed TypeScript workspaces (`fixtures/ts-workspaces/`): the pane shows the
    /// source definition from another package, labelled when a dist→source mapping found it.
    #[test]
    fn workspace_fixtures_show_cross_package_definitions() {
        use cho_fixtures::TsWorkspace;
        let cases = [
            (
                TsWorkspace::Dist,
                "packages/app/src/main.ts",
                "greet('x')",
                "packages/swap/src/greet.ts",
                Some("mapped (unverified): path swap"),
            ),
            (
                TsWorkspace::SourceCondition,
                "apps/web/src/main.ts",
                "openSession('a')",
                "packages/core/src/session.ts",
                None,
            ),
        ];
        for (w, path, needle, def_path, label) in cases {
            let sem = w.semantic();
            let dir = w.dir();
            let text = cho_core::fs::read_source_text_in(&dir, Path::new(path)).unwrap();
            let byte = text.find(needle).unwrap();
            let Answer::Symbol(c) = gather(&sem, &dir, path, byte, Some(&probe())) else {
                panic!("{}: a symbol", w.name());
            };
            assert_eq!(c.definitions.len(), 1, "{}", w.name());
            assert_eq!(c.definitions[0].path, def_path, "{}", w.name());
            assert_eq!(c.definitions[0].mapped.as_deref(), label, "{}", w.name());
            assert!(
                c.references.iter().any(|f| f.path == path),
                "{}: the use in {path} is listed",
                w.name()
            );
        }
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Semantic) {
        let dir_guard = tempfile::tempdir().unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/a.rs"),
            "fn helper() {}\nfn c() { helper(); }\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/b.rs"), "fn d() {\n    helper();\n}\n").unwrap();
        let docs = vec![
            Document {
                path: "src/a.rs".into(),
                encoding: Encoding::Utf8,
                occurrences: vec![occ(0, 3, 9, true), occ(1, 9, 15, false)],
            },
            Document {
                path: "src/b.rs".into(),
                encoding: Encoding::Utf8,
                occurrences: vec![occ(1, 4, 10, false)],
            },
        ];
        let mut infos = HashMap::new();
        infos.insert(
            SymbolId::new(HELPER, ""),
            SymbolInfo {
                display_name: "helper".into(),
                kind: "Function".into(),
                ..SymbolInfo::default()
            },
        );
        let sem = Semantic::new(ScipIndex::new(docs, infos), &dir, IndexStatus::Current);
        (dir_guard, dir, sem)
    }

    #[test]
    fn definition_and_references_grouped_by_file_with_previews() {
        let (_guard, dir, sem) = fixture();
        let Answer::Symbol(c) = gather(&sem, &dir, "src/a.rs", 26, Some(&probe())) else {
            panic!("expected a symbol");
        };
        assert_eq!((c.name.as_str(), c.kind.as_str()), ("helper", "Function"));
        assert_eq!(c.definitions.len(), 1);
        assert_eq!(c.definitions[0].preview, "fn helper() {}");
        assert_eq!((c.definitions[0].line, c.definitions[0].byte), (1, 3));
        assert_eq!(c.reference_count, 2);
        let files: Vec<_> = c.references.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(files, ["src/a.rs", "src/b.rs"]);
        assert_eq!(c.references[1].places[0].preview, "helper();");
        // The marks are the index's ranges within the trimmed previews.
        assert_eq!(c.definitions[0].mark, Some(3..9));
        assert_eq!(c.references[1].places[0].mark, Some(0..6));
        // Previews carry syntax spans: `fn` is a keyword, `helper` a function.
        let key_at = |p: &Place, needle: &str| {
            let at = p.preview.find(needle).unwrap();
            p.spans
                .iter()
                .find(|(r, _)| r.contains(&at))
                .map(|(_, k)| *k)
        };
        assert_eq!(key_at(&c.definitions[0], "fn"), Some("keyword"));
        assert_eq!(key_at(&c.definitions[0], "helper"), Some("function"));
        // Reference lines are coloured later, once on screen.
        assert!(c.references[1].places[0].spans.is_empty());
        let spans = highlight_line("src/b.rs", "let x = helper();");
        assert!(spans.iter().any(|(_, k)| *k == "keyword"));
        assert_eq!(c.omitted, 0);
    }

    #[test]
    fn no_symbol_under_the_caret() {
        let (_guard, dir, sem) = fixture();
        assert_eq!(gather(&sem, &dir, "src/a.rs", 1, None), Answer::NoSymbol);
    }

    #[test]
    fn readable_name_without_symbol_info() {
        let id = SymbolId::new("rust-analyzer cargo core 1.0 option/Option#map().", "");
        assert_eq!(display_from_symbol(&id), "map");
    }
}
