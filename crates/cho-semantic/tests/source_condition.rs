//! A workspace that resolves packages through a source export condition (`development`)
//! and emits per-file declarations into `.types/` with project references, on a real
//! scip-typescript index (`fixtures/ts-workspaces/source-condition`, see its README).
//! Cross-package navigation lands on source without any mapping, and the dist→source join
//! stays idle; if the indexer ever named the `.types` declarations instead, the join would
//! cover them.

use cho_semantic::index::{Document, Encoding, Occurrence, RawRange};
use cho_semantic::{IndexStatus, MapMethod, Mapping, ScipIndex, Semantic, SymbolId};
use std::path::{Path, PathBuf};

const WEB: &str = "apps/web/src/main.ts";

fn fixture() -> PathBuf {
    cho_fixtures::TsWorkspace::SourceCondition.dir()
}

fn index() -> ScipIndex {
    ScipIndex::load(&fixture().join("index.scip"), "").unwrap()
}

fn load() -> Semantic {
    Semantic::new(index(), fixture(), IndexStatus::Current).with_dist_links()
}

fn sym(s: &str) -> SymbolId {
    SymbolId::new(&format!("scip-typescript npm {s}"), "")
}

fn byte_of(path: &str, needle: &str) -> usize {
    cho_core::fs::read_source_text_in(&fixture(), Path::new(path))
        .unwrap()
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} in {path}"))
}

#[test]
fn imports_from_other_packages_land_on_source() {
    let sem = load();
    let cases = [
        ("openSession('a')", "packages/core/src/session.ts", 6),
        ("Session = openSession", "packages/core/src/session.ts", 1),
        // `new Client(…)` is a use of the constructor.
        ("Client(s.id)", "packages/client/src/index.ts", 6),
        ("button(await", "packages/ui/src/components/button.ts", 3),
    ];
    for (needle, path, line) in cases {
        let at = sem.symbol_at(WEB, byte_of(WEB, needle)).expect(needle);
        assert!(
            !at.id.symbol.contains(".d.ts"),
            "{needle}: {}",
            at.id.symbol
        );
        let defs = sem.definitions(&at.id);
        assert_eq!(defs.len(), 1, "{needle}");
        assert_eq!(
            (defs[0].path.as_str(), defs[0].line),
            (path, line),
            "{needle}"
        );
        assert!(
            defs[0].mapped.is_none(),
            "{needle}: found by the index, not mapped"
        );
    }
}

#[test]
fn references_cross_packages_without_mapping() {
    let sem = load();
    let refs = sem.references(&sym("@fx/core 0.0.0 src/`session.ts`/openSession()."));
    let mut paths: Vec<&str> = refs.iter().map(|l| l.path.as_str()).collect();
    paths.dedup();
    assert_eq!(
        paths,
        [
            "packages/core/src/session.ts",
            "apps/web/src/main.ts",
            "packages/client/src/index.ts",
        ]
    );
    assert!(refs.iter().all(|l| l.mapped.is_none()));
}

#[test]
fn the_join_finds_the_workspace_and_has_nothing_to_do() {
    let sem = load();
    let s = sem.dist_links().stats();
    assert_eq!(
        s.packages, 4,
        "web, client, core and ui; typescript-config has no indexed source"
    );
    assert_eq!(s.candidates, 0, "no symbol names a .d.ts file");
    assert!(sem.dist_links().is_empty());
}

/// If scip-typescript followed project references to the emitted declarations, as
/// command-line `tsc` does, uses would name `.types/src/x.d.ts` (`` `.types`/ `` in a symbol,
/// since `.` is not an identifier character). The join maps those back:
/// through the declaration map where there is one, through tsconfig `rootDir` where not.
#[test]
fn declarations_in_types_would_map_back_to_source() {
    let occ = |line: u32, s: &str| Occurrence {
        range: RawRange {
            start_line: line,
            start_col: 0,
            end_line: line,
            end_col: 1,
        },
        symbol: format!("scip-typescript npm {s}"),
        roles: 0,
    };
    let redirected = [
        "@fx/core 0.0.0 `.types`/src/`session.d.ts`/openSession().",
        "@fx/core 0.0.0 `.types`/src/`session.d.ts`/Session#id.",
        "@fx/core 0.0.0 `.types`/src/util/`retry.d.ts`/retry().",
        "@fx/ui 0.0.0 `.types`/components/`button.d.ts`/button().",
    ];
    let mut index = index();
    index.merge(ScipIndex::new(
        vec![Document {
            path: "apps/web/src/redirected.ts".into(),
            encoding: Encoding::Utf16,
            occurrences: redirected
                .iter()
                .enumerate()
                .map(|(i, s)| occ(i as u32, s))
                .collect(),
        }],
        Default::default(),
    ));
    let sem = Semantic::new(index, fixture(), IndexStatus::Current).with_dist_links();
    let expect = [
        ("packages/core/src/session.ts", MapMethod::DeclarationMap),
        ("packages/core/src/session.ts", MapMethod::DeclarationMap),
        ("packages/core/src/util/retry.ts", MapMethod::DeclarationMap),
        ("packages/ui/src/components/button.ts", MapMethod::Tsconfig),
    ];
    for (s, (path, method)) in redirected.iter().zip(expect) {
        let defs = sem.definitions(&sym(s));
        assert_eq!(defs.len(), 1, "{s}");
        assert_eq!(defs[0].path, path, "{s}");
        assert_eq!(
            defs[0].mapped.as_ref().map(|m| m.mapping),
            Some(Mapping {
                method,
                through_reexports: false
            }),
            "{s}"
        );
    }
}
