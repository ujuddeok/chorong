#![no_main]
//! SCIP index loading, queries and the dist→source join over a small workspace on disk.
//!
//! The first byte picks the mode: odd decodes the rest as raw `index.scip` bytes (must
//! decode or error, never panic); even builds an index from a vocabulary of paths and
//! symbols, writes fuzzed workspace files (manifest, tsconfig, declaration map, sources)
//! and checks that every answer is attributed to a document that really has that symbol,
//! lies inside the file on char boundaries, and that every dist→source link joins an
//! undefined declaration-file symbol to the same package's source definition.
use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use cho_semantic::dist_link::parse_symbol;
use cho_semantic::index::OccRef;
use cho_semantic::workspace::is_declaration_file;
use cho_semantic::{IndexOrigin, IndexStatus, ScipIndex, Semantic, SymbolId};
use protobuf::{EnumOrUnknown, Message};
use scip::types::{Document, Index, Occurrence, SymbolInformation};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Files of the fixture workspace, written fresh for each input.
const FILES: [&str; 9] = [
    "a.rs",
    "packages/a/src/index.ts",
    "packages/a/src/util.ts",
    "packages/a/dist/index.d.ts",
    "packages/a/package.json",
    "packages/a/tsconfig.json",
    "packages/a/dist/index.d.ts.map",
    "packages/a/.types/src/index.d.ts",
    "packages/a/.types/src/index.d.ts.map",
];

/// Per file, the shapes it can start from. The second manifest and tsconfig are the
/// source-condition layout (`docs/typescript-workspaces.md`): a `development` export
/// condition to source, JavaScript-only `dist`, and per-file declarations in `.types/`.
const TEMPLATES: [&[&str]; 9] = [
    &["fn foo() {}\r\nstruct Föo;\n"],
    &["export * from './util';\nexport class Foo { bar() {} }\n"],
    &["export class Foo {\n  bar(): void {}\n}\n"],
    &["export * from './util';\nexport declare class Foo { bar(): void; }\n"],
    &[
        r#"{"name":"@x/a","version":"1.0.0","types":"dist/index.d.ts","source":"src/index.ts"}"#,
        r#"{"name":"@x/a","version":"1.0.0","exports":{".":{"development":"./src/index.ts","default":"./dist/index.mjs"}}}"#,
    ],
    &[
        "{ // c\n \"compilerOptions\": {\"outDir\": \"dist\", \"rootDir\": \"src\",},\n}",
        r#"{"extends":"@x/config/library.json","compilerOptions":{"outDir":".types","rootDir":"."}}"#,
    ],
    &[r#"{"version":3,"sources":["../src/index.ts"],"mappings":""}"#],
    &["export * from './util';\nexport declare class Foo { bar(): void; }\n"],
    &[r#"{"version":3,"sources":["../../src/index.ts"],"mappings":""}"#],
];

const DOC_PATHS: [&str; 13] = [
    "packages/a/.types/src/index.d.ts",
    "a.rs",
    "src/index.ts",
    "src/util.ts",
    "dist/index.d.ts",
    "packages/a/src/index.ts",
    "packages/a/dist/index.d.ts",
    "../a.rs",
    "/a.rs",
    "./a.rs",
    "a.rs/",
    "",
    "packages//a/src/util.ts",
];

const PREFIXES: [&str; 3] = ["", "packages/a", "packages/a/"];

const SYMBOLS: [&str; 12] = [
    "scip-typescript npm @x/a 1.0.0 `.types`/src/`index.d.ts`/Foo#",
    "scip-typescript npm @x/a 1.0.0 `.types`/src/`index.d.ts`/Foo#bar().",
    "local 0",
    "local 1",
    "rust-analyzer cargo a 0.1.0 foo().",
    "scip-typescript npm @x/a 1.0.0 src/`index.ts`/Foo#",
    "scip-typescript npm @x/a 1.0.0 src/`util.ts`/Foo#",
    "scip-typescript npm @x/a 1.0.0 src/`util.ts`/Foo#bar().",
    "scip-typescript npm @x/a 1.0.0 dist/`index.d.ts`/Foo#",
    "scip-typescript npm @x/a 1.0.0 dist/`index.d.ts`/Foo#bar().",
    "scip-typescript npm @x/a 2.0.0 dist/`index.d.ts`/Foo#",
    "scip-typescript npm @x/a 1.0.0 dist/`index.d.ts`/`util.ts`/Foo#",
];

fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let r = std::env::temp_dir().join(format!("cho-fuzz-scip-{}", std::process::id()));
        std::fs::create_dir_all(r.join("packages/a/src")).unwrap();
        std::fs::create_dir_all(r.join("packages/a/dist")).unwrap();
        std::fs::create_dir_all(r.join("packages/a/.types/src")).unwrap();
        std::fs::write(
            r.join("package.json"),
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        r
    })
}

fn pick<'a, T>(u: &mut Unstructured<'_>, items: &'a [T]) -> Result<&'a T> {
    Ok(&items[u.choose_index(items.len())?])
}

fn text(u: &mut Unstructured<'_>, template: &str) -> Result<String> {
    Ok(match u.int_in_range(0..=3u8)? {
        0 => u.arbitrary::<String>()?,
        // The template with a fuzzed fragment spliced in at a char boundary.
        1 => {
            let mut at = u.int_in_range(0..=template.len())?;
            while !template.is_char_boundary(at) {
                at -= 1;
            }
            let frag: String = u.arbitrary()?;
            format!("{}{frag}{}", &template[..at], &template[at..])
        }
        _ => template.to_string(),
    })
}

fn symbol(u: &mut Unstructured<'_>) -> Result<String> {
    if u.ratio(1u8, 8)? {
        u.arbitrary()
    } else {
        Ok(pick(u, &SYMBOLS)?.to_string())
    }
}

fn build(u: &mut Unstructured<'_>) -> Result<(Index, String, Vec<String>)> {
    let mut contents = vec![];
    for (f, shapes) in FILES.iter().zip(TEMPLATES) {
        let shape = *pick(u, shapes)?;
        let c = text(u, shape)?;
        std::fs::write(root().join(f), &c).unwrap();
        contents.push(c);
    }
    let prefix = pick(u, &PREFIXES)?.to_string();
    let mut index = Index::new();
    for _ in 0..u.int_in_range(0..=4)? {
        let mut d = Document::new();
        d.relative_path = pick(u, &DOC_PATHS)?.to_string();
        d.position_encoding = EnumOrUnknown::from_i32(u.int_in_range(0..=3)?);
        for _ in 0..u.int_in_range(0..=8)? {
            let mut o = Occurrence::new();
            let n = u.int_in_range(0..=5)?;
            for _ in 0..n {
                o.range.push(if u.ratio(1u8, 16)? {
                    u.arbitrary()?
                } else {
                    i32::from(u.int_in_range(0..=6u8)?)
                });
            }
            o.symbol = symbol(u)?;
            o.symbol_roles = if u.ratio(1u8, 16)? {
                u.arbitrary()?
            } else {
                u.int_in_range(0..=1)?
            };
            d.occurrences.push(o);
        }
        for _ in 0..u.int_in_range(0..=2)? {
            let mut s = SymbolInformation::new();
            s.symbol = symbol(u)?;
            s.display_name = u.arbitrary()?;
            s.documentation = vec![u.arbitrary()?];
            d.symbols.push(s);
        }
        index.documents.push(d);
    }
    if u.arbitrary()? {
        let mut s = SymbolInformation::new();
        s.symbol = symbol(u)?;
        s.display_name = "Foo".into();
        index.external_symbols.push(s);
    }
    Ok((index, prefix, contents))
}

/// The fixture file a document path reads, as `Semantic` would read it (`None` when it
/// reads nothing).
fn file_for<'a>(path: &str, contents: &'a [String]) -> Option<&'a str> {
    let p = Path::new(path);
    if !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let key: Vec<_> = p
        .components()
        .map(|c| c.as_os_str().to_str().unwrap())
        .collect();
    let key = key.join("/");
    FILES
        .iter()
        .position(|f| *f == key)
        .map(|i| contents[i].as_str())
}

fn check_range(r: &std::ops::Range<usize>, text: &str, what: &str) {
    assert!(
        r.start <= r.end && r.end <= text.len(),
        "{what}: {r:?} len {}",
        text.len()
    );
    assert!(
        text.is_char_boundary(r.start) && text.is_char_boundary(r.end),
        "{what}: {r:?} not on char boundaries"
    );
}

/// Does document `path` have an occurrence of `id` (local symbols only in their own file)?
fn has_occurrence(index: &ScipIndex, id: &SymbolId, path: &str) -> bool {
    if id.local_in.as_deref().is_some_and(|l| l != path) {
        return false;
    }
    index.documents_for(path).iter().any(|&d| {
        index.documents()[d]
            .occurrences
            .iter()
            .any(|o| o.symbol == id.symbol)
    })
}

fn structured(data: &[u8]) -> Result<()> {
    let mut u = Unstructured::new(data);
    let (raw, prefix, contents) = build(&mut u)?;
    let bytes = raw.write_to_bytes().unwrap();
    let index = ScipIndex::from_scip_bytes(&bytes, &prefix).expect("own encoding decodes");

    // The lookup tables agree with the documents.
    for (di, d) in index.documents().iter().enumerate() {
        assert!(index.documents_for(&d.path).contains(&di));
    }
    for (id, occs) in index.symbols() {
        for (&at, def) in occs
            .definitions
            .iter()
            .map(|a| (a, true))
            .chain(occs.references.iter().map(|a| (a, false)))
        {
            let OccRef { doc, .. } = at;
            let o = index.occurrence(at);
            assert_eq!(o.symbol, id.symbol);
            assert_eq!(o.is_definition(), def);
            let path = &index.document(doc).path;
            assert_eq!(
                id.local_in.as_ref().is_some(),
                id.symbol.starts_with("local ")
            );
            if let Some(l) = &id.local_in {
                assert_eq!(l, path, "local symbol attributed across documents");
            }
        }
    }

    let paths: Vec<String> = {
        let mut p: Vec<String> = index.documents().iter().map(|d| d.path.clone()).collect();
        p.sort();
        p.dedup();
        p
    };
    let ids: Vec<SymbolId> = index.symbols().map(|(id, _)| id.clone()).collect();
    let origins = vec![IndexOrigin {
        project_root: prefix.trim_end_matches('/').to_string(),
        commit: None,
        status: IndexStatus::Current,
    }];
    let sem = Semantic::new(index, root(), IndexStatus::Current)
        .with_origins(origins)
        .with_dist_links();
    let index = sem.index();

    for path in &paths {
        let file = file_for(path, &contents);
        let occs = sem.document_occurrences(path);
        let Some(text) = file else {
            assert!(occs.is_empty(), "occurrences for unreadable {path:?}");
            assert!(sem.symbol_at(path, 0).is_none());
            continue;
        };
        for o in &occs {
            check_range(&o.range, text, "document occurrence");
            assert!(has_occurrence(index, &o.id, path));
        }
        let step = (text.len() / 24).max(1);
        for b in (0..=text.len()).step_by(step) {
            if let Some(s) = sem.symbol_at(path, b) {
                check_range(&s.range, text, "symbol_at");
                assert!(s.range.contains(&b), "symbol_at {b} gave {:?}", s.range);
                assert!(
                    has_occurrence(index, &s.id, path),
                    "{:?} not in {path}",
                    s.id
                );
            }
            let _ = sem.hover_at_with_peek(path, b);
        }
    }

    let check_locations = |id: &SymbolId, locs: Vec<cho_semantic::Location>| {
        for l in locs {
            let text = file_for(&l.path, &contents)
                .unwrap_or_else(|| panic!("location in unreadable {:?}", l.path));
            check_range(&l.range, text, "location");
            match &l.mapped {
                None => assert!(has_occurrence(index, id, &l.path), "{id:?} at {}", l.path),
                Some(m) => {
                    assert!(!m.verified);
                    let owner = SymbolId::new(&m.symbol, &l.path);
                    assert!(has_occurrence(index, &owner, &l.path));
                    // The asked symbol and the occurrence's own are joined by a link.
                    let links = sem.dist_links();
                    let joined = links.source_of(id).is_some_and(|(s, _)| *s == owner)
                        || links.source_of(&owner).is_some_and(|(s, _)| s == id);
                    assert!(joined, "mapped {owner:?} for {id:?} without a link");
                }
            }
        }
    };
    for id in &ids {
        check_locations(id, sem.definitions(id));
        check_locations(id, sem.references(id));
        let _ = sem.hover(id);
    }
    let _ = sem.search("foo", 10);

    for (dist, src, _) in sem.dist_links().links() {
        let d = index
            .occurrences_of(dist)
            .expect("linked symbol is in the index");
        assert!(d.definitions.is_empty(), "{dist:?} has its own definition");
        let s = index
            .occurrences_of(src)
            .expect("source symbol is in the index");
        assert!(!s.definitions.is_empty(), "{src:?} has no definition");
        let (pd, ps) = (
            parse_symbol(&dist.symbol).unwrap(),
            parse_symbol(&src.symbol).unwrap(),
        );
        assert_eq!(pd.name, ps.name, "linked across packages");
        assert_eq!(pd.rest, ps.rest, "linked to a different declaration");
        assert!(is_declaration_file(&pd.file));
        assert!(!is_declaration_file(&ps.file), "{src:?} is not source");
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    if mode & 1 == 1 {
        if let Ok(index) = ScipIndex::from_scip_bytes(rest, "p") {
            for (id, occs) in index.symbols() {
                for &at in occs.definitions.iter().chain(&occs.references) {
                    assert_eq!(index.occurrence(at).symbol, id.symbol);
                }
            }
            let sem = Semantic::new(index, root(), IndexStatus::Current).with_dist_links();
            let _ = sem.search("a", 5);
        }
        return;
    }
    let _ = structured(rest);
});
