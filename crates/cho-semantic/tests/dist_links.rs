//! Built declarations (`dist/*.d.ts`) joined to their source, on a real scip-typescript
//! index of a small pnpm workspace (`fixtures/ts-workspaces/dist`, see its README).

use cho_semantic::{IndexStatus, MapMethod, Mapping, ScipIndex, Semantic, SymbolId};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    cho_fixtures::TsWorkspace::Dist.dir()
}

fn load() -> Semantic {
    let dir = fixture();
    let index = ScipIndex::load(&dir.join("index.scip"), "").unwrap();
    Semantic::new(index, &dir, IndexStatus::Current).with_dist_links()
}

fn sym(s: &str) -> SymbolId {
    SymbolId::new(&format!("scip-typescript npm {s}"), "")
}

fn mapping(method: MapMethod, through_reexports: bool) -> Mapping {
    Mapping {
        method,
        through_reexports,
    }
}

/// `(path, line, mapping)` of each definition.
fn defs(sem: &Semantic, id: &SymbolId) -> Vec<(String, usize, Option<Mapping>)> {
    sem.definitions(id)
        .into_iter()
        .map(|l| (l.path, l.line, l.mapped.map(|m| m.mapping)))
        .collect()
}

#[test]
fn each_method_lands_on_the_source_definition() {
    let sem = load();
    let cases = [
        (
            "@fx/maps 1.0.0 dist/`shapes.d.ts`/square().",
            "packages/maps/lib/geometry.ts",
            5,
            mapping(MapMethod::DeclarationMap, false),
        ),
        (
            "@fx/maps 1.0.0 dist/`shapes.d.ts`/Shape#area().",
            "packages/maps/lib/geometry.ts",
            2,
            mapping(MapMethod::DeclarationMap, false),
        ),
        (
            "@fx/tscfg 2.1.0 out/`calc.d.ts`/add().",
            "packages/tscfg/code/calc.ts",
            1,
            mapping(MapMethod::Tsconfig, false),
        ),
        (
            "@fx/entry 0.3.0 types/`index.d.ts`/Client#fetch().",
            "packages/entry/main/entry.ts",
            4,
            mapping(MapMethod::SourceField, false),
        ),
        (
            "@fx/swap 1.0.0 dist/`greet.d.ts`/greet().",
            "packages/swap/src/greet.ts",
            1,
            mapping(MapMethod::PathSwap, false),
        ),
        (
            "@fx/bundle 1.0.0 dist/`index.d.ts`/format().",
            "packages/bundle/src/format.ts",
            3,
            mapping(MapMethod::PathSwap, true),
        ),
        (
            "@fx/bundle 1.0.0 dist/`index.d.ts`/User#name.",
            "packages/bundle/src/models/user.ts",
            2,
            mapping(MapMethod::PathSwap, true),
        ),
    ];
    for (dist, path, line, how) in cases {
        let id = sym(dist);
        assert_eq!(
            defs(&sem, &id),
            vec![(path.to_string(), line, Some(how))],
            "{dist}"
        );
    }
}

#[test]
fn unchecked_links_are_not_made() {
    let sem = load();
    // Declared only in dist: the source file has no `Missing`.
    let missing = sym("@fx/swap 1.0.0 dist/`greet.d.ts`/Missing.");
    assert!(sem.index().occurrences_of(&missing).is_some());
    assert!(sem.mapped_source(&missing).is_none());
    assert!(sem.definitions(&missing).is_empty());
    // Renamed by the bundle (`export { parse as read }`): the descriptors differ, so the
    // source definition can't be checked by name chain.
    let renamed = sym("@fx/bundle 1.0.0 dist/`index.d.ts`/parse().");
    assert!(sem.index().occurrences_of(&renamed).is_some());
    assert!(sem.definitions(&renamed).is_empty());
}

#[test]
fn references_are_the_union_attributed_to_their_symbol() {
    let sem = load();
    let source = sym("@fx/maps 1.0.0 lib/`geometry.ts`/square().");
    let dist = sym("@fx/maps 1.0.0 dist/`shapes.d.ts`/square().");
    let from_source = sem.references(&source);
    let from_dist = sem.references(&dist);
    let summary = |v: &[cho_semantic::Location]| {
        v.iter()
            .map(|l| {
                (
                    l.path.clone(),
                    l.is_definition,
                    l.mapped.as_ref().map(|m| m.symbol.clone()),
                )
            })
            .collect::<Vec<_>>()
    };
    // The definition, then app's import and call: those belong to the dist symbol.
    assert_eq!(
        summary(&from_source),
        vec![
            ("packages/maps/lib/geometry.ts".into(), true, None),
            (
                "packages/app/src/main.ts".into(),
                false,
                Some(dist.symbol.clone())
            ),
            (
                "packages/app/src/main.ts".into(),
                false,
                Some(dist.symbol.clone())
            ),
        ]
    );
    // From the dist symbol: the same places, the source ones marked instead.
    assert_eq!(
        summary(&from_dist),
        vec![
            (
                "packages/maps/lib/geometry.ts".into(),
                true,
                Some(source.symbol.clone())
            ),
            ("packages/app/src/main.ts".into(), false, None),
            ("packages/app/src/main.ts".into(), false, None),
        ]
    );
    assert!(
        from_dist
            .iter()
            .all(|l| l.mapped.as_ref().is_none_or(|m| !m.verified))
    );
    // Highlights in a file follow the union too.
    assert_eq!(
        sem.occurrences_in(&source, "packages/app/src/main.ts")
            .len(),
        2
    );
}

#[test]
fn position_lookups_from_another_package_reach_source() {
    let sem = load();
    let main = cho_core::fs::read_source_text_in(
        &fixture(),
        std::path::Path::new("packages/app/src/main.ts"),
    )
    .unwrap();
    let byte = main.find("greet('x')").unwrap();
    let at = sem.symbol_at("packages/app/src/main.ts", byte).unwrap();
    assert_eq!(
        at.id,
        sym("@fx/swap 1.0.0 dist/`greet.d.ts`/greet()."),
        "scip-typescript names the built declaration"
    );
    let d = sem.definitions(&at.id);
    assert_eq!(d[0].path, "packages/swap/src/greet.ts");
    let hover = sem.hover_at("packages/app/src/main.ts", byte).unwrap();
    let sig = hover.signature.expect("the source's signature");
    assert!(
        sig.text.contains("greet(name: string): string"),
        "{}",
        sig.text
    );
    let m = hover.mapped.expect("marked as mapped");
    assert_eq!(m.mapping.label(), "mapped (unverified): path swap");
    assert!(!m.verified);
    assert_eq!(
        hover.definitions[0].location.path,
        "packages/swap/src/greet.ts"
    );
}

#[test]
fn a_mapped_reference_does_not_verify_an_edge() {
    let sem = load();
    let source = sym("@fx/swap 1.0.0 src/`greet.ts`/greet().");
    let main = cho_core::fs::read_source_text_in(
        &fixture(),
        std::path::Path::new("packages/app/src/main.ts"),
    )
    .unwrap();
    assert!(!sem.references_within(&source, "packages/app/src/main.ts", 0..main.len()));
}

#[test]
fn stats_count_candidates_and_links() {
    let sem = load();
    let s = sem.dist_links().stats();
    assert_eq!(s.packages, 6);
    let links = sem.dist_links().links();
    assert_eq!(s.mapped, links.len());
    assert!(
        s.candidates > s.mapped,
        "Missing and the renamed parse stay unmapped"
    );
    for (dist, source, _) in links {
        assert!(dist.symbol.contains(".d.ts`/"), "{}", dist.symbol);
        assert!(!source.symbol.contains(".d.ts`/"), "{}", source.symbol);
    }
}

#[test]
fn other_versions_and_packages_are_not_joined() {
    use cho_semantic::index::{Document, Encoding, Occurrence, RawRange};
    let dir = fixture();
    let index = ScipIndex::load(&dir.join("index.scip"), "").unwrap();
    // A use of the same file and name at another version, and of a package that isn't in
    // the workspace (a node_modules dependency): neither is a workspace build.
    let occ = |s: &str| Occurrence {
        range: RawRange {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 1,
        },
        symbol: format!("scip-typescript npm {s}"),
        roles: 0,
    };
    let extra = ScipIndex::new(
        vec![Document {
            path: "packages/app/src/other.ts".into(),
            encoding: Encoding::Utf16,
            occurrences: vec![
                occ("@fx/swap 9.9.9 dist/`greet.d.ts`/greet()."),
                occ("left-pad 1.3.0 dist/`index.d.ts`/leftPad()."),
            ],
        }],
        Default::default(),
    );
    let mut index = index;
    index.merge(extra);
    let sem = Semantic::new(index, &dir, IndexStatus::Current).with_dist_links();
    assert!(
        sem.mapped_source(&sym("@fx/swap 9.9.9 dist/`greet.d.ts`/greet()."))
            .is_none()
    );
    assert!(
        sem.mapped_source(&sym("left-pad 1.3.0 dist/`index.d.ts`/leftPad()."))
            .is_none()
    );
    assert!(
        sem.mapped_source(&sym("@fx/swap 1.0.0 dist/`greet.d.ts`/greet()."))
            .is_some()
    );
}

#[test]
fn without_a_workspace_nothing_is_joined() {
    let dir = fixture();
    let index = ScipIndex::load(&dir.join("index.scip"), "").unwrap();
    // Rooted inside one package: no workspace file there, so no package is known.
    let sem =
        Semantic::new(index, dir.join("packages/app"), IndexStatus::Current).with_dist_links();
    assert!(sem.dist_links().is_empty());
}
