//! Tests over hand-built indexes: source files in a temp directory plus a `ScipIndex`
//! assembled from named occurrences. No indexer, no GUI.

use super::*;
use cho_semantic::{Document, Encoding, IndexStatus, Occurrence, RawRange, ScipIndex, Semantic};
use std::collections::HashMap;
use std::path::PathBuf;

const CORE_LIB: &str = "pub struct Session { id: u32 }\npub fn create_session() -> Session { Session { id: 1 } }\npub fn helper() {}\npub trait Store { fn put(&self); }\n";
const APP_LIB: &str = "use core::create_session;\npub fn handle() { let s = create_session(); log(); }\npub fn log() { handle(); }\npub fn other() { unknown_fn(); tokio::spawn(); }\n";
const APP_TEST: &str = "fn t() { handle(); }\n";

fn sym(pkg: &str, d: &str) -> String {
    format!("rust-analyzer cargo {pkg} 0.1.0 {d}")
}

pub(crate) struct Fx {
    dir: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `(needle, nth, symbol, is_definition)` -> an occurrence at the nth match of `needle`.
type Spec<'a> = (&'a str, usize, String, bool);

fn doc(path: &str, text: &str, specs: &[Spec]) -> Document {
    let occurrences = specs
        .iter()
        .map(|(needle, nth, symbol, def)| {
            let start = text
                .match_indices(needle)
                .nth(*nth)
                .unwrap_or_else(|| panic!("{needle}"))
                .0;
            let line = text[..start].matches('\n').count();
            let col = start - text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
            Occurrence {
                range: RawRange {
                    start_line: line as u32,
                    start_col: col as u32,
                    end_line: line as u32,
                    end_col: (col + needle.len()) as u32,
                },
                symbol: symbol.clone(),
                roles: i32::from(*def),
            }
        })
        .collect();
    Document {
        path: path.into(),
        encoding: Encoding::Utf8,
        occurrences,
    }
}

pub(crate) fn fixture(name: &str) -> (Fx, Semantic) {
    fixture_with(name, &[])
}

/// The fixture, with extra occurrences in `app/src/lib.rs`.
fn fixture_with(name: &str, extra: &[Spec]) -> (Fx, Semantic) {
    let dir = std::env::temp_dir().join(format!("cho-graph-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (p, t) in [
        ("core/Cargo.toml", "[package]\nname = \"core\"\n"),
        ("app/Cargo.toml", "[package]\nname = \"app\"\n"),
        ("core/src/lib.rs", CORE_LIB),
        ("app/src/lib.rs", APP_LIB),
        ("app/tests/it.rs", APP_TEST),
    ] {
        let full = dir.join(p);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, t).unwrap();
    }
    let session = sym("core", "Session#");
    let create = sym("core", "create_session().");
    let helper = sym("core", "helper().");
    let store = sym("core", "Store#");
    let handle = sym("app", "handle().");
    let log = sym("app", "log().");
    let other = sym("app", "other().");
    let t = sym("app", "t().");
    let spawn = sym("tokio", "spawn().");
    let docs = vec![
        doc(
            "core/src/lib.rs",
            CORE_LIB,
            &[
                ("Session", 0, session.clone(), true),
                ("create_session", 0, create.clone(), true),
                ("Session", 1, session.clone(), false),
                ("Session", 2, session.clone(), false),
                ("helper", 0, helper.clone(), true),
                ("Store", 0, store.clone(), true),
            ],
        ),
        doc(
            "app/src/lib.rs",
            APP_LIB,
            &[
                ("create_session", 0, create.clone(), false),
                ("handle", 0, handle.clone(), true),
                ("create_session", 1, create.clone(), false),
                ("log", 0, log.clone(), false),
                ("log", 1, log.clone(), true),
                ("handle", 1, handle.clone(), false),
                ("other", 0, other.clone(), true),
                ("spawn", 0, spawn.clone(), false),
            ]
            .into_iter()
            .chain(extra.iter().cloned())
            .collect::<Vec<_>>(),
        ),
        doc(
            "app/tests/it.rs",
            APP_TEST,
            &[
                ("t", 0, t.clone(), true),
                ("handle", 0, handle.clone(), false),
            ],
        ),
    ];
    let index = ScipIndex::new(docs, HashMap::new());
    (
        Fx { dir: dir.clone() },
        Semantic::new(index, dir, IndexStatus::Current),
    )
}

fn prov() -> Provenance {
    Provenance {
        repo: "local:/work/repo".into(),
        worktree: ".".into(),
        branch: Some("feature/x".into()),
        commit: Some("abc1234".into()),
        dirty: true,
        shared: false,
    }
}

pub(crate) fn spec() -> ViewSpec {
    ViewSpec::new(prov())
}

pub(crate) fn id(g: &RefGraph, q: &str) -> u32 {
    resolve_symbol(g, q).unwrap_or_else(|e| panic!("{e}"))
}

fn edge<'a>(g: &'a Graph, from: &str, to: &str, kind: EdgeKind) -> Option<&'a Edge> {
    g.edges
        .iter()
        .find(|e| e.from.contains(from) && e.to.contains(to) && e.kind == kind)
}

#[test]
fn edges_get_kinds_from_syntax_and_endpoints_from_the_index() {
    let (_fx, sem) = fixture("kinds");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.file_level = true;
    s.depth = 1;
    s.direction = Direction::In;
    let graph = neighbourhood(&g, &[id(&g, "core:create_session")], &s);
    assert!(edge(&graph, "handle", "create_session", EdgeKind::Calls).is_some());
    // The `use` line is outside every declaration: a file-level source, not a guess.
    let e = edge(
        &graph,
        "filelevel:app/src/lib.rs",
        "create_session",
        EdgeKind::References,
    )
    .unwrap();
    assert_eq!(e.status, EdgeStatus::Verified);
    // `-> Session` is a type use and `Session { .. }` constructs.
    let mut s = spec();
    s.depth = 1;
    s.direction = Direction::In;
    let sess = neighbourhood(&g, &[id(&g, "Session")], &s);
    assert!(edge(&sess, "create_session", "Session", EdgeKind::TypeUses).is_some());
    assert!(edge(&sess, "create_session", "Session", EdgeKind::Constructs).is_some());
}

#[test]
fn depth_limits_the_walk_and_directions_differ() {
    let (_fx, sem) = fixture("depth");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.direction = Direction::In;
    s.depth = 1;
    let d1 = neighbourhood(&g, &[id(&g, "create_session")], &s);
    assert!(
        d1.node("sym:rust-analyzer cargo app 0.1.0 handle().")
            .is_some()
    );
    assert!(d1.node("sym:rust-analyzer cargo app 0.1.0 t().").is_none());
    s.depth = 2;
    let d2 = neighbourhood(&g, &[id(&g, "create_session")], &s);
    assert!(d2.node("sym:rust-analyzer cargo app 0.1.0 t().").is_some());
    s.direction = Direction::Out;
    let out = neighbourhood(&g, &[id(&g, "handle")], &s);
    assert!(edge(&out, "handle", "create_session", EdgeKind::Calls).is_some());
    assert!(out.node("sym:rust-analyzer cargo app 0.1.0 t().").is_none());
}

#[test]
fn cycles_are_kept_and_flagged() {
    let (_fx, sem) = fixture("cycle");
    let g = RefGraph::build(&sem);
    let graph = neighbourhood(&g, &[id(&g, "handle")], &spec());
    let a = edge(&graph, "handle", "log", EdgeKind::Calls).unwrap();
    let b = edge(&graph, "log", "handle", EdgeKind::Calls).unwrap();
    assert!(a.in_cycle && b.in_cycle);
    let c = edge(&graph, "handle", "create_session", EdgeKind::Calls).unwrap();
    assert!(!c.in_cycle);
}

#[test]
fn node_cap_makes_a_more_node() {
    let (_fx, sem) = fixture("cap");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.file_level = true;
    s.direction = Direction::In;
    s.depth = 3;
    s.max_nodes = 2;
    let graph = neighbourhood(&g, &[id(&g, "create_session")], &s);
    assert!(graph.view.truncated);
    let more = graph
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::More)
        .expect("more node");
    assert!(more.label.starts_with("+1 more in "), "{}", more.label);
    assert_eq!(more.hidden, Some(1));
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.to == more.id || e.from == more.id)
    );
}

#[test]
fn externals_collapse_to_one_node_and_are_opt_in() {
    let (_fx, sem) = fixture("ext");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.depth = 1;
    s.direction = Direction::Out;
    let without = neighbourhood(&g, &[id(&g, "other")], &s);
    assert!(without.nodes.iter().all(|n| n.kind != NodeKind::External));
    s.externals = true;
    let with = neighbourhood(&g, &[id(&g, "other")], &s);
    let ext: Vec<_> = with
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::External)
        .collect();
    assert_eq!(ext.len(), 1);
    assert_eq!(ext[0].label, "tokio");
    assert!(edge(&with, "other", "ext:tokio", EdgeKind::Calls).is_some());
}

#[test]
fn unresolved_references_into_a_repository_package_sit_inside_it() {
    // `tokio` in `other` resolves to a symbol of package `core` the index has no definition for.
    let gone = sym("core", "gone().");
    let (_fx, sem) = fixture_with("inrepo", &[("tokio", 0, gone, false)]);
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.depth = 1;
    s.direction = Direction::Out;
    s.externals = true;
    let graph = neighbourhood(&g, &[id(&g, "other")], &s);
    let ext = graph.node("ext:core").expect("unresolved node");
    assert_eq!(ext.kind, NodeKind::External);
    assert!(ext.unresolved_in_repo);
    assert_eq!(ext.label, "core (unresolved)");
    assert_eq!(
        ext.parent.as_deref(),
        Some("pkg:core"),
        "drawn inside the package"
    );
    assert!(graph.node("pkg:core").is_some());
    assert_eq!(
        graph
            .nodes
            .iter()
            .filter(|n| n.label == "core" || n.label.starts_with("core "))
            .count(),
        2,
        "one package node and its unresolved part"
    );
    let r = graph
        .rollups
        .package
        .iter()
        .find(|r| r.from == "pkg:app" && r.to == "pkg:core")
        .expect("rolls up into the package");
    assert_eq!(r.references, 1);
}

#[test]
fn shared_assets_load_by_a_relative_path() {
    let base = std::env::temp_dir().join(format!("cho-graph-assets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = install_shared_assets(&base.join("cache")).unwrap();
    assert!(dir.ends_with(assets_folder_name()));
    for f in [
        "cytoscape.min.js",
        "elk.bundled.js",
        "LICENSE-cytoscape-MIT.txt",
        "LICENSE-elkjs-EPL-2.0.md",
    ] {
        assert!(dir.join(f).is_file(), "{f}");
    }
    // Installing again leaves the files as they are.
    assert_eq!(install_shared_assets(&base.join("cache")).unwrap(), dir);
    let page = base.join("out dir/page.html");
    let Some(Assets::Shared(prefix)) = shared_assets_for(&page, &dir) else {
        panic!("relative path expected");
    };
    assert_eq!(prefix, format!("../cache/{}/", assets_folder_name()));
    // Pages in the cache get a copy in their own folder, loaded downward only (Safari refuses
    // a `file://` page's scripts from a parent folder).
    let graphs = base.join("cache/graphs");
    let beside = crate::html::shared_beside(&graphs.join("page.html"), &graphs);
    assert_eq!(beside, Assets::Shared(format!("{}/", assets_folder_name())));
    assert!(
        graphs
            .join(assets_folder_name())
            .join("elk.bundled.js")
            .is_file()
    );
    let (_fx, sem) = fixture("assets");
    let rg = RefGraph::build(&sem);
    let g = neighbourhood(&rg, &[id(&rg, "handle")], &spec());
    let html = to_html(
        &g,
        &PageOptions {
            theme: None,
            assets: Assets::Shared(prefix.clone()),
        },
    );
    assert!(html.contains(&format!("<script src=\"{prefix}elk.bundled.js\"></script>")));
    assert!(html.contains("EPL-2.0") && html.contains("MIT"));
    assert!(
        html.len() < 100_000,
        "libraries are not inlined: {}",
        html.len()
    );
    let inline = to_html(&g, &PageOptions::default());
    assert!(inline.len() > 1_000_000 && !inline.contains("<script src="));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn unresolved_calls_are_listed_never_matched() {
    let (_fx, sem) = fixture("unres");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.depth = 1;
    s.direction = Direction::Out;
    let graph = neighbourhood(&g, &[id(&g, "other")], &s);
    let other = graph
        .node("sym:rust-analyzer cargo app 0.1.0 other().")
        .unwrap();
    let names: Vec<_> = other
        .unresolved_calls
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["unknown_fn"]);
    assert!(graph.edges.iter().all(|e| !e.to.contains("unknown_fn")));
}

#[test]
fn rollups_count_references_and_symbols() {
    let (_fx, sem) = fixture("rollup");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.file_level = true;
    s.direction = Direction::In;
    s.depth = 2;
    let graph = neighbourhood(&g, &[id(&g, "create_session")], &s);
    let r = graph
        .rollups
        .package
        .iter()
        .find(|r| r.from == "pkg:app" && r.to == "pkg:core")
        .expect("app -> core");
    assert_eq!(r.references, 2);
    assert_eq!(r.symbols, 1);
    assert_eq!(r.kinds["calls"], 1);
    assert!(graph.rollups.package.iter().all(|r| r.from != r.to));
}

#[test]
fn outbound_closure_reaches_packages_at_any_depth() {
    let (_fx, sem) = fixture("closure");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.depth = 8;
    let graph = outbound_closure(&g, &[id(&g, "other")], &s);
    assert_eq!(graph.view.kind, ViewKind::OutboundClosure);
    assert_eq!(graph.view.default_level, Level::Package);
    let v: serde_json::Value = serde_json::from_str(&graph.to_json()).unwrap();
    assert_eq!(
        (
            &v["view"]["kind"],
            &v["view"]["default_level"],
            &v["view"]["direction"]
        ),
        (&"outbound-closure".into(), &"package".into(), &"out".into())
    );
    assert!(
        graph
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::External && n.label == "tokio")
    );
    let mut s = spec();
    s.depth = 8;
    let from_t = outbound_closure(&g, &[id(&g, "t")], &s);
    assert!(
        from_t.node("pkg:core").is_some(),
        "t -> handle -> create_session"
    );
}

#[test]
fn blast_radius_marks_reachable_tests() {
    let (_fx, sem) = fixture("blast");
    let g = RefGraph::build(&sem);
    let changed = changed_decls(
        &g,
        &[
            Hunk {
                path: "core/src/lib.rs".into(),
                start_line: 2,
                end_line: 2,
            },
            Hunk {
                path: "gone/other.rs".into(),
                start_line: 1,
                end_line: 1,
            },
        ],
    );
    assert_eq!(changed.decls.len(), 1);
    assert_eq!(changed.unindexed, ["gone/other.rs"]);
    let mut s = spec();
    s.depth = 3;
    let graph = blast_radius(&g, &changed, &["core/src/old.rs".into()], &s, vec![]);
    assert_eq!(graph.reachable_tests, ["app/tests/it.rs"]);
    assert!(
        graph
            .notes
            .iter()
            .any(|n| n.contains("reachability, not coverage"))
    );
    assert!(graph.notes.iter().any(|n| n.contains("merge base")));
    assert!(graph.notes.contains(
        &"Deleted in the diff (their edges are not in a head-only index): core/src/old.rs.".into()
    ));
    let none = blast_radius(&g, &Changed::default(), &[], &s, vec!["front end".into()]);
    assert_eq!(none.notes[0], "front end");
    assert_eq!(none.notes[1], "No indexed declaration overlaps the diff.");
    assert!(graph.node("pkg:app").is_some());
}

#[test]
fn changed_decls_picks_the_innermost_declaration() {
    let (_fx, sem) = fixture("inner");
    let g = RefGraph::build(&sem);
    // Line 4 of core is `trait Store { fn put }`; `put` has no SCIP symbol here, so the trait.
    let c = changed_decls(
        &g,
        &[Hunk {
            path: "core/src/lib.rs".into(),
            start_line: 4,
            end_line: 4,
        }],
    );
    assert_eq!(c.decls, [id(&g, "Store")]);
    // A hunk past every declaration is file-level.
    let c = changed_decls(
        &g,
        &[Hunk {
            path: "core/src/lib.rs".into(),
            start_line: 40,
            end_line: 41,
        }],
    );
    assert!(c.decls.is_empty());
    assert_eq!(c.file_level.len(), 1);
}

#[test]
fn symbol_resolution_never_guesses() {
    let (_fx, sem) = fixture("resolve");
    let g = RefGraph::build(&sem);
    assert!(resolve_symbol(&g, "core:create_session").is_ok());
    assert!(resolve_symbol(&g, "core/src/lib.rs#helper").is_ok());
    assert!(resolve_symbol(&g, "rust-analyzer cargo core 0.1.0 helper().").is_ok());
    assert!(matches!(
        resolve_symbol(&g, "nope"),
        Err(ResolveError::NotFound(_))
    ));
    assert!(matches!(
        resolve_symbol(&g, "app:create_session"),
        Err(ResolveError::NotFound(_))
    ));
}

#[test]
fn edge_kind_filter_hides_kinds() {
    let (_fx, sem) = fixture("filter");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.direction = Direction::In;
    s.depth = 1;
    s.kinds = vec![EdgeKind::TypeUses];
    let graph = neighbourhood(&g, &[id(&g, "Session")], &s);
    assert!(graph.edges.iter().all(|e| e.kind == EdgeKind::TypeUses));
    assert!(!graph.edges.is_empty());
}

#[test]
fn links_carry_rev_branch_and_dirty_and_pages_are_self_contained() {
    let (fx, sem) = fixture("html");
    let g = RefGraph::build(&sem);
    let graph = neighbourhood(&g, &[id(&g, "create_session")], &spec());
    let n = graph
        .node("sym:rust-analyzer cargo core 0.1.0 create_session().")
        .unwrap();
    let link = n.link.as_deref().unwrap();
    assert!(
        link.contains("rev=abc1234")
            && link.contains("branch=feature/x")
            && link.ends_with("dirty=1&from=graph"),
        "{link}"
    );
    assert!(link.contains("symbol=create_session"));
    assert!(
        n.open_cmd
            .as_deref()
            .unwrap()
            .starts_with("cho open 'chorong://open?")
    );
    let parsed = cho_core::ChorongLink::parse(link).unwrap();
    assert_eq!(
        format!("{}&from=graph", parsed.to_url()),
        link,
        "from=graph is the only parameter ChorongLink does not model"
    );

    let html = to_html(&graph, &PageOptions::default());
    for needle in [
        "--bg:",
        "--k-calls:",
        "prefers-color-scheme: light",
        "feature/x",
        "abc1234",
        "uncommitted changes",
        "Static references only",
    ] {
        assert!(html.contains(needle), "{needle}");
    }
    assert!(
        !html.contains("src=\"http") && !html.contains("href=\"http"),
        "no network"
    );
    assert!(
        !html.contains(fx.dir.to_str().unwrap()),
        "no temp path in the page"
    );
    assert!(html.contains("EPL-2.0") && html.contains("Cytoscape"));
    // Dark and light come from the two bundled Chorong themes.
    let dark = cho_theme::Theme::bundled(cho_theme::Appearance::Dark);
    let c = dark.editor_background;
    assert!(html.contains(&format!("--bg:#{:02x}{:02x}{:02x}", c.r, c.g, c.b)));
}

#[test]
fn pages_and_links_exported_for_sharing_name_only_the_repository_and_commit() {
    let (_fx, sem) = fixture("share");
    let g = RefGraph::build(&sem);
    let mut p = prov().for_sharing();
    p.repo = "github:acme/api".into();
    let graph = neighbourhood(&g, &[id(&g, "create_session")], &ViewSpec::new(p));
    let n = graph
        .node("sym:rust-analyzer cargo core 0.1.0 create_session().")
        .unwrap();
    let link = n.link.as_deref().unwrap();
    assert!(
        link.starts_with("chorong://open?repo=github:acme/api&rev=abc1234"),
        "{link}"
    );
    assert!(
        !link.contains("branch=") && !link.contains("dirty="),
        "{link}"
    );
    assert!(
        n.web_url
            .as_deref()
            .unwrap()
            .starts_with("https://github.com/acme/api/blob/abc1234/")
    );
    let html = to_html(&graph, &PageOptions::default());
    assert!(html.contains("exported for sharing") && html.contains("abc1234"));
    for gone in ["feature/x", "uncommitted changes", "worktree <b>"] {
        assert!(!html.contains(gone), "{gone}");
    }
    let v: serde_json::Value = serde_json::from_str(&graph.to_json()).unwrap();
    assert!(v["meta"].get("worktree").is_none());
    assert_eq!(v["meta"]["shared"], true);
    assert!(
        to_dot(&graph)
            .lines()
            .next()
            .unwrap()
            .ends_with("exported for sharing")
    );
}

#[test]
fn json_and_dot_describe_the_same_graph() {
    let (_fx, sem) = fixture("outputs");
    let g = RefGraph::build(&sem);
    let graph = neighbourhood(&g, &[id(&g, "handle")], &spec());
    let v: serde_json::Value = serde_json::from_str(&graph.to_json()).unwrap();
    assert_eq!(v["meta"]["schema"], "cho-graph/1");
    assert_eq!(v["meta"]["branch"], "feature/x");
    assert_eq!(v["meta"]["dirty"], true);
    assert_eq!(v["meta"]["index_status"], "current");
    assert!(
        v["meta"].get("repo").is_none(),
        "link base stays out of the data"
    );
    assert_eq!(v["edges"].as_array().unwrap().len(), graph.edges.len());
    let dot = to_dot(&graph);
    assert!(dot.starts_with("// Neighbourhood of handle"));
    assert!(dot.contains("digraph cho") && dot.contains("color=red") && dot.contains("calls x1"));
    assert_eq!(dot.matches("->").count(), graph.edges.len());
}

/// The `dist` TypeScript workspace fixture: a pnpm workspace whose `app` uses other packages through
/// their built declarations (`dist`). The graph draws those uses to the source
/// declarations, labelled `mapped`.
#[test]
fn uses_through_built_declarations_reach_source_and_say_mapped() {
    let sem = cho_fixtures::TsWorkspace::Dist.semantic();
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.depth = 1;
    s.direction = Direction::In;
    let graph = neighbourhood(&g, &[id(&g, "greet")], &s);
    let e = edge(&graph, "run", "greet", EdgeKind::Calls).expect("app's call reaches source");
    assert_eq!(e.status, EdgeStatus::Mapped);
    assert_eq!(e.mapped, e.count);
    assert!(graph.notes.iter().any(|n| n.contains("mapped")));
    // `Missing` exists only in dist: not drawn to anything in swap's source.
    let out = neighbourhood(&g, &[id(&g, "run")], &{
        let mut s = spec();
        s.depth = 1;
        s.direction = Direction::Out;
        s
    });
    assert!(out.nodes.iter().all(|n| n.label != "Missing"));
}

/// Found by the `graph_views` fuzz target: a branch name with a `'` (git allows it) ended the
/// quote in the `cho open '…'` command shown on a node, so what followed ran as shell.
#[test]
fn open_command_stays_one_argument_for_a_branch_with_a_quote() {
    let (_fx, sem) = fixture("quote");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.provenance.branch = Some("x';touch pwned;'".into());
    let graph = neighbourhood(&g, &[id(&g, "core:helper")], &s);
    let n = graph
        .node("sym:rust-analyzer cargo core 0.1.0 helper().")
        .unwrap();
    let (link, cmd) = (n.link.as_ref().unwrap(), n.open_cmd.as_ref().unwrap());
    assert!(link.contains("branch=x'"), "{link}");
    assert_eq!(*cmd, format!("cho open '{}'", link.replace('\'', "'\\''")));
    assert!(cmd.contains("'\\'';touch"), "{cmd}");
}

/// Found by the `graph_views` fuzz target: the DOT header comment carried the branch name and
/// the file path as they were, so one with a line break turned the rest of it into DOT.
#[test]
fn dot_header_comment_stays_on_one_line() {
    let (_fx, sem) = fixture("dotcomment");
    let g = RefGraph::build(&sem);
    let mut s = spec();
    s.provenance.branch = Some("a\nb [label=\"x\"];\r}".into());
    s.provenance.worktree = "w\u{2028}x".into();
    let graph = neighbourhood(&g, &[id(&g, "core:helper")], &s);
    let dot = to_dot(&graph);
    assert_eq!(
        dot.lines().filter(|l| l.starts_with("//")).count(),
        2,
        "{dot}"
    );
    assert!(!dot.contains("\nb [label"), "{dot}");
    assert!(!dot.contains('\r'));
}

/// A graph from a partial or failed index says so in its notes, as a stale one does.
#[test]
fn notes_name_a_stale_partial_or_failed_index() {
    let (fx, sem) = fixture("status-notes");
    for (status, says) in [
        (IndexStatus::Stale, "The index is stale"),
        (IndexStatus::Partial, "The index is partial"),
        (IndexStatus::Failed, "The last index build failed"),
    ] {
        let mut g = RefGraph::build(&sem);
        g.status = status;
        let graph = neighbourhood(&g, &[id(&g, "handle")], &spec());
        assert!(
            graph.notes.iter().any(|n| n.starts_with(says)),
            "{status}: {:?}",
            graph.notes
        );
    }
    let g = RefGraph::build(&sem);
    let graph = neighbourhood(&g, &[id(&g, "handle")], &spec());
    assert!(
        !graph
            .notes
            .iter()
            .any(|n| n.starts_with("The index is") || n.starts_with("The last index")),
        "{:?}",
        graph.notes
    );
    drop(fx);
}
