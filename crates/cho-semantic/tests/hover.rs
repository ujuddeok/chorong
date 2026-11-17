//! Hover queries over a hand-built index and real files, no indexer needed.
#![allow(clippy::disallowed_methods)] // tests read their own fixtures

use cho_semantic::index::{Document, Encoding, ScipIndex, Signature, SymbolId, SymbolInfo};
use cho_semantic::{IndexOrigin, IndexStatus, NoSignature, Semantic, SignatureSource};
use std::collections::HashMap;
use std::path::PathBuf;

mod common;
use common::occ;

const ADD: &str = "rust-analyzer cargo p 0.1 add().";
const MONEY: &str = "scip-typescript npm web HEAD src/`m.ts`/money().";
const BARE: &str = "rust-analyzer cargo p 0.1 bare().";
const EXTERNAL: &str = "rust-analyzer cargo std 1.0 vec/Vec#";

/// `lib/src/a.rs` (Rust, signatures in `signature_documentation`) and `web/src/m.ts`
/// (TypeScript, signature as the first documentation entry), in two projects.
fn fixture(name: &str) -> (PathBuf, Semantic) {
    let dir = std::env::temp_dir().join(format!("cho-hover-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("lib/src")).unwrap();
    std::fs::create_dir_all(dir.join("web/src")).unwrap();
    std::fs::write(
        dir.join("lib/src/a.rs"),
        "fn add() {}\nfn bare() { add(); let v = Vec::new(); v; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("web/src/m.ts"),
        "function money() {}\nconst total = money();\n",
    )
    .unwrap();
    let docs = vec![
        Document {
            path: "lib/src/a.rs".into(),
            encoding: Encoding::Utf8,
            occurrences: vec![
                occ(0, 3, 6, ADD, true),
                occ(1, 3, 7, BARE, true),
                occ(1, 12, 15, ADD, false),
                occ(1, 23, 24, "local 0", true),
                occ(1, 27, 30, EXTERNAL, false),
                occ(1, 39, 40, "local 0", false),
            ],
        },
        Document {
            path: "web/src/m.ts".into(),
            encoding: Encoding::Utf16,
            occurrences: vec![
                occ(0, 9, 14, MONEY, true),
                occ(1, 6, 11, "local 1", true),
                occ(1, 14, 19, MONEY, false),
            ],
        },
    ];
    let mut infos = HashMap::new();
    infos.insert(
        SymbolId::new(ADD, ""),
        SymbolInfo {
            display_name: "add".into(),
            kind: "Function".into(),
            documentation: vec!["Adds things.".into()],
            signature: Some(Signature {
                text: "fn add()".into(),
                language: "rust".into(),
            }),
            ..SymbolInfo::default()
        },
    );
    infos.insert(
        SymbolId::new(BARE, ""),
        SymbolInfo {
            display_name: "bare".into(),
            kind: "Function".into(),
            ..SymbolInfo::default()
        },
    );
    infos.insert(
        SymbolId::new(MONEY, ""),
        SymbolInfo {
            kind: "Function".into(),
            documentation: vec![
                "```ts\nfunction money(): void\n```".into(),
                "Makes money.".into(),
            ],
            ..SymbolInfo::default()
        },
    );
    infos.insert(
        SymbolId::new("local 1", "web/src/m.ts"),
        SymbolInfo {
            kind: "Constant".into(),
            documentation: vec!["```ts\nvar total: void\n```".into()],
            ..SymbolInfo::default()
        },
    );
    let origins = vec![
        IndexOrigin {
            project_root: "lib".into(),
            commit: Some("aaa".into()),
            status: IndexStatus::Current,
        },
        IndexOrigin {
            project_root: "web".into(),
            commit: Some("bbb".into()),
            status: IndexStatus::Stale,
        },
    ];
    let sem =
        Semantic::new(ScipIndex::new(docs, infos), &dir, IndexStatus::Stale).with_origins(origins);
    (dir, sem)
}

fn byte_of(dir: &std::path::Path, path: &str, needle: &str) -> usize {
    std::fs::read_to_string(dir.join(path))
        .unwrap()
        .find(needle)
        .unwrap()
}

#[test]
fn signature_documentation_is_kept_with_docs_and_definition() {
    let (dir, sem) = fixture("rust");
    let call = byte_of(&dir, "lib/src/a.rs", "add();");
    let h = sem.hover_at("lib/src/a.rs", call + 1).unwrap();
    assert_eq!(h.id, SymbolId::new(ADD, ""));
    assert_eq!(h.range, Some(call..call + 3));
    assert_eq!((h.name.as_str(), h.kind.as_str()), ("add", "Function"));
    let sig = h.signature.unwrap();
    assert_eq!(
        (sig.text.as_str(), sig.language.as_str()),
        ("fn add()", "rust")
    );
    assert_eq!(sig.source, SignatureSource::SignatureDocumentation);
    assert_eq!(h.documentation, vec!["Adds things.".to_string()]);
    assert_eq!(h.definitions.len(), 1);
    assert_eq!(
        (
            h.definitions[0].location.line,
            h.definitions[0].location.column
        ),
        (1, 4)
    );
    assert_eq!(h.local_text, "add");
    assert_eq!(h.index.commit.as_deref(), Some("aaa"));
    assert_eq!(h.index.status, IndexStatus::Current);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_lone_code_block_in_documentation_is_the_signature() {
    let (dir, sem) = fixture("ts");
    let call = byte_of(&dir, "web/src/m.ts", "money();");
    let h = sem.hover_at("web/src/m.ts", call).unwrap();
    let sig = h.signature.unwrap();
    assert_eq!(
        (sig.text.as_str(), sig.language.as_str(), sig.source),
        (
            "function money(): void",
            "ts",
            SignatureSource::Documentation
        )
    );
    assert_eq!(h.documentation, vec!["Makes money.".to_string()]);
    assert_eq!(
        h.name, "money",
        "empty display name: the symbol ID's last descriptor"
    );
    assert_eq!(h.index.commit.as_deref(), Some("bbb"));
    assert_eq!(h.index.status, IndexStatus::Stale);

    let local = byte_of(&dir, "web/src/m.ts", "total");
    let h = sem.hover_at("web/src/m.ts", local).unwrap();
    assert!(h.id.is_local());
    assert_eq!(h.signature.unwrap().text, "var total: void");
    assert_eq!(h.name, "total", "locals use the source text");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn missing_signatures_are_reported_never_synthesised() {
    let (dir, sem) = fixture("none");
    let bare = byte_of(&dir, "lib/src/a.rs", "bare");
    let h = sem.hover_at("lib/src/a.rs", bare).unwrap();
    assert_eq!(h.signature, Err(NoSignature::NotRecorded));

    // A local with occurrences but no symbol information.
    let v = byte_of(&dir, "lib/src/a.rs", "v =");
    let h = sem.hover_at("lib/src/a.rs", v).unwrap();
    assert_eq!(h.signature, Err(NoSignature::NoSymbolInformation));
    assert_eq!(h.name, "v", "the source text under the occurrence");
    assert_eq!(h.local_text, "v");
    assert_eq!(h.kind, "");

    // An external symbol: referenced, never defined in the index.
    let vec = byte_of(&dir, "lib/src/a.rs", "Vec::");
    let h = sem.hover_at("lib/src/a.rs", vec).unwrap();
    assert!(h.definitions.is_empty());
    assert_eq!(h.signature, Err(NoSignature::NoSymbolInformation));

    assert!(sem.hover_at("lib/src/a.rs", 0).is_none(), "`fn` keyword");
    assert!(sem.hover(&SymbolId::new("pkg nowhere.", "")).is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn hover_by_symbol_without_a_definition_uses_the_overall_index() {
    let (dir, sem) = fixture("overall");
    let h = sem.hover(&SymbolId::new(EXTERNAL, "")).unwrap();
    assert_eq!(h.range, None);
    assert_eq!(h.index.commit, None, "projects disagree on the commit");
    assert_eq!(h.index.status, IndexStatus::Stale);
    let h = sem.hover(&SymbolId::new(ADD, "")).unwrap();
    assert_eq!(h.index.commit.as_deref(), Some("aaa"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_renamed_import_has_both_names() {
    let dir = std::env::temp_dir().join(format!("cho-hover-rename-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/m.ts"), "export function add() {}\n").unwrap();
    std::fs::write(
        dir.join("src/u.ts"),
        "import { add as plus } from './m';\nplus();\n",
    )
    .unwrap();
    let sym = "scip-typescript npm web HEAD src/`m.ts`/add().";
    let docs = vec![
        Document {
            path: "src/m.ts".into(),
            encoding: Encoding::Utf8,
            occurrences: vec![occ(0, 16, 19, sym, true)],
        },
        Document {
            path: "src/u.ts".into(),
            encoding: Encoding::Utf8,
            occurrences: vec![occ(0, 9, 12, sym, false), occ(1, 0, 4, sym, false)],
        },
    ];
    let sem = Semantic::new(
        ScipIndex::new(docs, HashMap::new()),
        &dir,
        IndexStatus::Current,
    );
    let h = sem.hover_at("src/u.ts", 10).unwrap();
    assert_eq!((h.name.as_str(), h.local_text.as_str()), ("add", "add"));
    let h = sem.hover_at("src/u.ts", 24 + 11 + 1).unwrap();
    assert_eq!((h.name.as_str(), h.local_text.as_str()), ("add", "plus"));
    // By symbol there is no occurrence, hence no local text.
    let h = sem.hover(&SymbolId::new(sym, "")).unwrap();
    assert_eq!((h.name.as_str(), h.local_text.as_str()), ("add", ""));
    std::fs::remove_dir_all(dir).ok();
}

const PEEK_RS: &str = "pub struct Point {\n    pub x: i32,\n}\n\nimpl Point {\n    pub fn norm(&self) -> i32 {\n        self.x\n    }\n}\n\npub const LIMIT: u32 = 10 + 1;\n";

/// A Rust file with a struct, a method and a constant, plus a Python file, a text
/// file.
fn peek_fixture(name: &str) -> (PathBuf, Semantic) {
    let dir = std::env::temp_dir().join(format!("cho-peek-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/p.rs"), PEEK_RS).unwrap();
    std::fs::write(dir.join("src/notes.txt"), "plain\n").unwrap();
    let at = |needle: &str| -> (u32, u32, u32) {
        let b = PEEK_RS.find(needle).unwrap();
        let line = PEEK_RS[..b].matches('\n').count();
        let col = b - PEEK_RS[..b].rfind('\n').map_or(0, |i| i + 1);
        (line as u32, col as u32, (col + needle.len()) as u32)
    };
    let d = |needle: &str, sym: &str| {
        let (l, c, e) = at(needle);
        occ(l, c, e, sym, true)
    };
    let docs = vec![
        Document {
            path: "src/p.rs".into(),
            encoding: Encoding::Utf8,
            occurrences: vec![
                d("Point", "r cargo p 1 Point#"),
                d("x", "r cargo p 1 Point#x."),
                d("norm", "r cargo p 1 Point#norm()."),
                d("LIMIT", "r cargo p 1 LIMIT."),
            ],
        },
        Document {
            path: "src/notes.txt".into(),
            encoding: Encoding::Utf8,
            occurrences: vec![occ(0, 0, 5, "r cargo p 1 note.", true)],
        },
    ];
    let sem = Semantic::new(
        ScipIndex::new(docs, HashMap::new()),
        &dir,
        IndexStatus::Current,
    );
    (dir, sem)
}

fn peek_of(sem: &Semantic, symbol: &str) -> (Option<String>, Option<cho_semantic::NoDeclaration>) {
    let mut h = sem.hover(&SymbolId::new(symbol, "")).unwrap();
    assert_eq!(h.definitions[0].peek, None, "not without a peek");
    sem.peek(&mut h);
    let d = &h.definitions[0];
    let text = d.declaration().map(|p| {
        let src = std::fs::read_to_string(sem.root().join(&d.location.path)).unwrap();
        src[p.range().clone()].to_string()
    });
    (text, d.no_declaration())
}

fn declaration_range(
    sem: &Semantic,
    loc: &cho_semantic::Location,
) -> Result<std::ops::Range<usize>, cho_semantic::NoDeclaration> {
    sem.peek_definition(loc).map(|p| p.range().clone())
}

#[test]
fn peek_finds_the_innermost_declaration() {
    let (dir, sem) = peek_fixture("decl");
    let (t, why) = peek_of(&sem, "r cargo p 1 Point#");
    assert_eq!(t.unwrap(), "pub struct Point {\n    pub x: i32,\n}");
    assert_eq!(why, None);
    let (t, _) = peek_of(&sem, "r cargo p 1 Point#norm().");
    assert_eq!(
        t.unwrap(),
        "pub fn norm(&self) -> i32 {\n        self.x\n    }"
    );
    let (t, _) = peek_of(&sem, "r cargo p 1 LIMIT.");
    assert_eq!(t.unwrap(), "pub const LIMIT: u32 = 10 + 1;");
    // A field is not an outline declaration: its struct is shown instead.
    let (t, why) = peek_of(&sem, "r cargo p 1 Point#x.");
    assert_eq!(t.unwrap(), "pub struct Point {\n    pub x: i32,\n}");
    assert_eq!(why, None);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn peek_says_why_there_is_no_range() {
    use cho_semantic::NoDeclaration::*;
    let (dir, sem) = peek_fixture("why");
    assert_eq!(
        peek_of(&sem, "r cargo p 1 note.").1,
        Some(UnsupportedLanguage)
    );
    // The index can name a file that is gone or lies outside the root.
    for path in ["src/gone.rs", "../x.rs"] {
        let loc = cho_semantic::Location {
            path: path.into(),
            range: 0..1,
            line: 1,
            column: 1,
            is_definition: true,
            mapped: None,
        };
        assert_eq!(declaration_range(&sem, &loc), Err(OutsideIndexedFiles));
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn hover_at_with_peek_matches_hover_at_plus_ranges() {
    let (dir, sem) = peek_fixture("with");
    let at = PEEK_RS.find("LIMIT").unwrap();
    let plain = sem.hover_at("src/p.rs", at).unwrap();
    assert_eq!(plain.definitions[0].peek, None);
    let peeked = sem.hover_at_with_peek("src/p.rs", at).unwrap();
    assert_eq!(
        peeked.definitions[0]
            .declaration()
            .map(|p| p.range().clone()),
        Some(at - 10..at + 20)
    );
    assert_eq!(peeked.name, plain.name);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn peek_is_cached_and_follows_file_changes() {
    let (dir, sem) = peek_fixture("cache");
    let h = sem.hover(&SymbolId::new("r cargo p 1 LIMIT.", "")).unwrap();
    let loc = &h.definitions[0].location;
    let first = declaration_range(&sem, loc).unwrap();
    // Same length, new mtime: re-parsed, still the same answer.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(dir.join("src/p.rs"), PEEK_RS).unwrap();
    assert_eq!(declaration_range(&sem, loc), Ok(first));
    // A different file: the old outline is not served.
    std::fs::write(dir.join("src/p.rs"), "// moved\n").unwrap();
    assert_eq!(
        declaration_range(&sem, loc),
        Err(cho_semantic::NoDeclaration::NoDeclaration)
    );
    std::fs::remove_dir_all(dir).ok();
}

/// Numbers for the report, not a gate (timing bounds fail on loaded machines):
/// `cargo test -p cho-semantic --release --test hover -- --ignored --nocapture timings`.
#[test]
#[ignore]
fn timings() {
    let (dir, sem) = peek_fixture("time");
    let at = PEEK_RS.find("LIMIT").unwrap();
    let n = 20_000u32;
    let t = std::time::Instant::now();
    for _ in 0..n {
        std::hint::black_box(sem.hover_at("src/p.rs", at));
    }
    let plain = t.elapsed() / n;
    let h = sem.hover_at("src/p.rs", at).unwrap();
    let loc = &h.definitions[0].location;
    let t = std::time::Instant::now();
    std::hint::black_box(declaration_range(&sem, loc).unwrap());
    let cold = t.elapsed();
    let t = std::time::Instant::now();
    for _ in 0..n {
        std::hint::black_box(declaration_range(&sem, loc).unwrap());
    }
    let warm = t.elapsed() / n;
    println!("hover_at {plain:?}; declaration_range cold {cold:?}, cached {warm:?}");
    // Generous bounds (debug builds, noisy machines); the printed numbers are the evidence.
    assert!(plain < std::time::Duration::from_micros(200));
    assert!(warm < std::time::Duration::from_micros(200));
    std::fs::remove_dir_all(dir).ok();
}
