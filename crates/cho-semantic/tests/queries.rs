//! Core query logic over a hand-built index and real files, no indexer needed.
#![allow(clippy::disallowed_methods)] // tests read their own fixtures

use cho_investigation::Investigation;
use cho_investigation::check::check_with_oracle;
use cho_semantic::index::{
    Document, Encoding, Occurrence, RawRange, ScipIndex, SymbolId, SymbolInfo,
};
use cho_semantic::{IndexStatus, Semantic};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod common;
use common::occ;

const HELPER: &str = "pkg helper().";
const CALLER: &str = "pkg caller().";
const OTHER: &str = "pkg other().";

fn info(name: &str, kind: &str) -> SymbolInfo {
    SymbolInfo {
        display_name: name.into(),
        kind: kind.into(),
        ..SymbolInfo::default()
    }
}

/// Files on disk plus an index describing them.
fn fixture(name: &str) -> (PathBuf, Semantic) {
    let dir = std::env::temp_dir().join(format!("cho-sem-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // line 0: helper def; line 1: caller def calling helper; line 2: other def, a local.
    std::fs::write(
        dir.join("src/a.rs"),
        "fn helper() {}\nfn caller() { helper(); }\nfn other() { let x = 1; x; }\n",
    )
    .unwrap();
    // UTF-16 document: "é" is 2 bytes but one unit; "😀" is 4 bytes, two units.
    std::fs::write(
        dir.join("src/b.rs"),
        "let é = helper(); let 😀 = helper();\nlet y = 2; y;\n",
    )
    .unwrap();
    let docs = vec![
        Document {
            path: "src/a.rs".into(),
            encoding: Encoding::Utf8,
            occurrences: vec![
                occ(0, 3, 9, HELPER, true),
                occ(1, 3, 9, CALLER, true),
                occ(1, 14, 20, HELPER, false),
                occ(2, 3, 8, OTHER, true),
                occ(2, 17, 18, "local 1", true),
                occ(2, 24, 25, "local 1", false),
                occ(9, 0, 3, "pkg stale().", false), // line doesn't exist: dropped
            ],
        },
        Document {
            path: "src/b.rs".into(),
            encoding: Encoding::Utf16,
            occurrences: vec![
                occ(0, 8, 14, HELPER, false),
                occ(0, 27, 33, HELPER, false),
                occ(1, 4, 5, "local 1", true),
                occ(1, 11, 12, "local 1", false),
            ],
        },
    ];
    let mut infos = HashMap::new();
    infos.insert(SymbolId::new(HELPER, ""), info("helper", "Function"));
    infos.insert(SymbolId::new(CALLER, ""), info("caller", "Function"));
    infos.insert(SymbolId::new(OTHER, ""), info("Other_Helper", "Function"));
    let sem = Semantic::new(ScipIndex::new(docs, infos), &dir, IndexStatus::Current);
    (dir, sem)
}

fn id(s: &str) -> SymbolId {
    SymbolId::new(s, "")
}

#[test]
fn symbol_at_and_definitions() {
    let (dir, sem) = fixture("sym");
    let src = std::fs::read_to_string(dir.join("src/a.rs")).unwrap();
    let call = src.find("helper();").unwrap();
    let at = sem.symbol_at("src/a.rs", call + 2).unwrap();
    assert_eq!(at.id, id(HELPER));
    assert!(!at.is_definition);
    assert_eq!(at.range, call..call + 6);
    let defs = sem.definitions(&at.id);
    assert_eq!(defs.len(), 1);
    assert_eq!(
        (defs[0].path.as_str(), defs[0].line, defs[0].column),
        ("src/a.rs", 1, 4)
    );
    assert!(defs[0].is_definition);
    assert!(
        sem.symbol_at("src/a.rs", 1).is_none(),
        "`fn` keyword has no symbol"
    );
    assert!(sem.symbol_at("src/missing.rs", 0).is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn multi_line_occurrences_are_not_symbols_under_the_cursor() {
    let (dir, sem) = fixture("multiline");
    let mut docs = sem.index().documents().to_vec();
    docs[0].occurrences.push(Occurrence {
        range: RawRange {
            start_line: 0,
            start_col: 0,
            end_line: 3,
            end_col: 0,
        },
        symbol: "pkg crate/".into(),
        roles: 1,
    });
    let sem = Semantic::new(
        ScipIndex::new(docs, HashMap::new()),
        &dir,
        IndexStatus::Current,
    );
    assert!(sem.symbol_at("src/a.rs", 1).is_none());
    assert_eq!(sem.definitions(&id("pkg crate/")).len(), 1);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn references_list_definitions_first_and_drop_misfits() {
    let (dir, sem) = fixture("refs");
    let refs = sem.references(&id(HELPER));
    let places: Vec<_> = refs
        .iter()
        .map(|l| (l.path.as_str(), l.line, l.is_definition))
        .collect();
    assert_eq!(
        places,
        [
            ("src/a.rs", 1, true),
            ("src/a.rs", 2, false),
            ("src/b.rs", 1, false),
            ("src/b.rs", 1, false)
        ]
    );
    // UTF-16 columns became byte offsets: the first call in b.rs starts after "let é = ".
    let b = std::fs::read_to_string(dir.join("src/b.rs")).unwrap();
    assert_eq!(&b[refs[2].range.clone()], "helper");
    assert_eq!(&b[refs[3].range.clone()], "helper");
    assert_eq!(refs[2].range.start, b.find("helper").unwrap());
    // `stale()` points at a line the file doesn't have, so it yields nothing.
    assert!(sem.references(&id("pkg stale().")).is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn locals_never_match_across_documents() {
    let (dir, sem) = fixture("local");
    let a = std::fs::read_to_string(dir.join("src/a.rs")).unwrap();
    let x = sem.symbol_at("src/a.rs", a.find("x;").unwrap()).unwrap();
    assert!(x.id.is_local());
    let refs = sem.references(&x.id);
    assert!(refs.iter().all(|l| l.path == "src/a.rs"), "{refs:?}");
    assert_eq!(refs.len(), 2);
    let b = std::fs::read_to_string(dir.join("src/b.rs")).unwrap();
    let y = sem.symbol_at("src/b.rs", b.find("y;").unwrap()).unwrap();
    assert_ne!(x.id, y.id);
    assert!(sem.references(&y.id).iter().all(|l| l.path == "src/b.rs"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn search_is_case_insensitive_definitions_only_and_ranked() {
    let (dir, sem) = fixture("search");
    let names: Vec<_> = sem
        .search("HELP", 10)
        .iter()
        .map(|h| h.display_name.clone())
        .collect();
    assert_eq!(names, ["helper", "Other_Helper"]); // prefix match before substring match
    assert_eq!(sem.search("helper", 1).len(), 1);
    assert!(sem.search("nothing", 10).is_empty());
    assert!(sem.search("", 10).is_empty());
    let hit = &sem.search("caller", 10)[0];
    assert_eq!((hit.kind.as_str(), hit.location.line), ("Function", 2));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn search_names_symbols_whose_index_has_no_display_name() {
    // scip-typescript records no display name; the name comes from the symbol's last descriptor.
    let (dir, sem) = fixture("search-noname");
    let ts = "t npm web 1 src/`m.ts`/union().";
    let mut docs = sem.index().documents().to_vec();
    docs[0].occurrences.push(occ(0, 0, 5, ts, true));
    let mut infos = HashMap::new();
    infos.insert(SymbolId::new(ts, ""), info("", "Function"));
    let sem = Semantic::new(ScipIndex::new(docs, infos), &dir, IndexStatus::Current);
    let hits = sem.search("UNI", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].display_name, "union");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn references_within_respects_ranges_and_files() {
    let (dir, sem) = fixture("within");
    let a = std::fs::read_to_string(dir.join("src/a.rs")).unwrap();
    let caller = a.find("fn caller").unwrap()..a.find("fn other").unwrap();
    let other = a.find("fn other").unwrap()..a.len();
    assert!(sem.references_within(&id(HELPER), "src/a.rs", caller));
    assert!(!sem.references_within(&id(HELPER), "src/a.rs", other));
    assert!(
        !sem.references_within(&id(HELPER), "src/a.rs", 0..8),
        "definition is not a reference"
    );
    std::fs::remove_dir_all(dir).ok();
}

fn investigation(edges: &str) -> Investigation {
    Investigation::parse(&format!(
        r#"{{ "chorong": 1, "title": "t", "kind": "diagram", "repo": "local:/x", "rev": "abcdef1",
          "nodes": [
            {{ "id": "caller", "anchor": {{ "path": "src/a.rs", "symbol": "caller" }} }},
            {{ "id": "helper", "anchor": {{ "path": "src/a.rs", "symbol": "helper" }} }},
            {{ "id": "other", "anchor": {{ "path": "src/a.rs", "symbol": "other" }} }},
            {{ "id": "file", "anchor": {{ "path": "src/a.rs" }} }}
          ], "edges": [{edges}] }}"#
    ))
    .unwrap()
}

#[test]
fn oracle_verifies_real_edges_and_leaves_wrong_ones_claimed() {
    let (dir, sem) = fixture("oracle");
    let inv = investigation(
        r#"{"from":"caller","to":"helper","rel":"calls"},
           {"from":"helper","to":"caller","rel":"calls"},
           {"from":"other","to":"helper","rel":"references"},
           {"from":"caller","to":"helper","rel":"implements"},
           {"from":"caller","to":"file","rel":"calls"},
           {"from":"file","to":"helper","rel":"dispatches"}"#,
    );
    let report = check_with_oracle(&inv, &dir, Some(&sem));
    let statuses: Vec<_> = report.edges.iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        [
            "verified", "claimed", "claimed", "claimed", "claimed", "verified"
        ]
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn rejects_paths_that_escape_the_root() {
    let (dir, sem) = fixture("escape");
    assert!(sem.symbol_at("../etc/passwd", 0).is_none());
    assert!(sem.symbol_at("/etc/passwd", 0).is_none());
    std::fs::remove_dir_all(dir).ok();
    let _ = Path::new("");
}

#[test]
fn symbol_at_with_many_occurrences_matches_each_one_exactly() {
    // 400 lines of `aaa bbb ccc` => 1200 occurrences, plus a nested pair on line 7
    // where the inner (smaller) occurrence must win.
    let dir = std::env::temp_dir().join(format!("cho-sem-many-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let text = "aaa bbb ccc\n".repeat(400);
    std::fs::write(dir.join("src/m.rs"), &text).unwrap();
    let mut occurrences = vec![];
    for line in 0..400u32 {
        for (n, col) in [0u32, 4, 8].into_iter().enumerate() {
            occurrences.push(occ(line, col, col + 3, &format!("pkg s{line}_{n}."), false));
        }
    }
    occurrences.push(occ(7, 0, 11, "pkg outer.", false));
    let docs = vec![Document {
        path: "src/m.rs".into(),
        encoding: Encoding::Utf8,
        occurrences,
    }];
    let sem = Semantic::new(
        ScipIndex::new(docs, HashMap::new()),
        &dir,
        IndexStatus::Current,
    );
    for line in [0usize, 1, 199, 398, 399] {
        for (n, col) in [0usize, 4, 8].into_iter().enumerate() {
            let start = line * 12 + col;
            for byte in start..start + 3 {
                let at = sem.symbol_at("src/m.rs", byte).unwrap();
                assert_eq!(at.id, id(&format!("pkg s{line}_{n}.")));
                assert_eq!(at.range, start..start + 3);
            }
            assert!(
                sem.symbol_at("src/m.rs", start + 3).is_none(),
                "the gap after a token has no symbol"
            );
        }
    }
    // Line 7: the outer occurrence covers the gaps; inner ones still win on tokens.
    assert_eq!(
        sem.symbol_at("src/m.rs", 7 * 12 + 3).unwrap().id,
        id("pkg outer.")
    );
    assert_eq!(
        sem.symbol_at("src/m.rs", 7 * 12 + 5).unwrap().id,
        id("pkg s7_1.")
    );
    std::fs::remove_dir_all(dir).ok();
}
