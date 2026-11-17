#![no_main]
//! Reference-graph views over a synthetic SCIP index.
//!
//! The fuzz bytes drive a small Rust workspace on disk (packages with odd names, files with
//! odd names, functions calling each other along chains, rings, stars, self-loops and dense
//! clusters, plus references to external and unknown symbols) and an index whose definitions
//! and references sit on the real byte ranges. Every view (neighbourhood, outbound closure,
//! file, package, blast radius over changed declarations) is checked: edge endpoints and
//! parents are nodes; node ids are unique; depth limits hold, nodes are at their shortest
//! distance and reachable from the start set; the node cap holds; edges exist in the graph
//! with the same counts and only of the asked kinds; cycle flags agree with an independent
//! reachability search; the same input gives the same JSON; `to_dot` keeps labels inside
//! quoted strings and comments on their own lines; `to_html` keeps node text out of markup
//! and the data block free of `<`; links are one quoted shell word.
use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use cho_graph::{
    Assets, Changed, Direction, EdgeKind, Graph, Hunk, NodeKind, PageOptions, Provenance, RefGraph,
    ResolveError, ViewSpec, blast_radius, changed_decls, file_view, neighbourhood,
    outbound_closure, package_view, resolve_symbol, to_dot, to_html,
};
use cho_semantic::{Document, Encoding, IndexStatus, Occurrence, RawRange, ScipIndex, Semantic};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const PACKAGES: [&str; 5] = ["p0", "p1", "a-b", "x</script><b>&", "q\"uote"];
const FILE_NAMES: [&str; 7] = [
    "src/a.rs",
    "src/b.rs",
    "src/<i>.rs",
    "src/q\"x.rs",
    "src/s&t.rs",
    "tests/t.rs",
    "src/a b.rs",
];

/// Memory-backed when the system has one: the fixture is rewritten for every input.
fn temp() -> PathBuf {
    let shm = PathBuf::from("/dev/shm");
    if shm.is_dir() {
        shm
    } else {
        std::env::temp_dir()
    }
}

fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let r = temp().join(format!("cho-fuzz-graph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(&r).unwrap();
        r
    })
}

/// A manifest outside the repository that a symlink inside it points to.
fn outside_manifest() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        let p = temp().join(format!(
            "cho-fuzz-graph-outside-{}.toml",
            std::process::id()
        ));
        std::fs::write(&p, "[package]\nname = 'leaked-outside'\n").unwrap();
        p
    })
}

struct Fn {
    file: usize,
    name: String,
    /// Symbols this function's body refers to, by the name written in it.
    calls: Vec<(String, String)>,
}

struct Fixture {
    index: ScipIndex,
}

fn symbol(pkg: &str, name: &str) -> String {
    format!("rust-analyzer cargo {pkg} 0.1.0 {name}().")
}

/// Writes the workspace and returns the index over it.
fn fixture(u: &mut Unstructured<'_>) -> Result<Fixture> {
    let r = root();
    for e in std::fs::read_dir(r).unwrap().flatten() {
        let _ = std::fs::remove_dir_all(e.path());
    }
    let nfiles = u.int_in_range(1..=5usize)?;
    let npkgs = u.int_in_range(1..=PACKAGES.len().min(3))?;
    let mut paths: Vec<(String, usize)> = vec![];
    for k in 0..nfiles {
        let pkg = u.int_in_range(0..=npkgs - 1)?;
        let mut rel = FILE_NAMES[u.choose_index(FILE_NAMES.len())?].to_string();
        let mut path = format!("{}/{rel}", PACKAGES[pkg]);
        if paths.iter().any(|(p, _)| *p == path) {
            rel = rel.replace(".rs", &format!("{k}.rs"));
            path = format!("{}/{rel}", PACKAGES[pkg]);
        }
        paths.push((path, pkg));
    }
    for (i, pkg) in PACKAGES.iter().enumerate().take(npkgs) {
        let m = r.join(pkg).join("Cargo.toml");
        std::fs::create_dir_all(m.parent().unwrap()).unwrap();
        std::fs::write(m, format!("[package]\nname = '{pkg}'\n")).unwrap();
        let _ = i;
    }
    // A manifest that is a symlink out of the repository must not name a package.
    let leak = u.ratio(1u8, 4)?;
    if leak {
        let dir = r.join("leak");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(outside_manifest(), dir.join("Cargo.toml"));
        paths.push(("leak/src/z.rs".to_string(), usize::MAX));
    }

    // Functions: names are global (`f0`, `f1`, ...), each in one file.
    let nfns = u.int_in_range(0..=40usize)?;
    let mut fns: Vec<Fn> = (0..nfns)
        .map(|j| Fn {
            file: u.int_in_range(0..=paths.len() - 1).unwrap_or(0),
            name: format!("f{j}"),
            calls: vec![],
        })
        .collect();
    let pkg_of = |file: usize| -> &str {
        match paths[file].1 {
            usize::MAX => "leak",
            p => PACKAGES[p],
        }
    };
    let own_sym = |fns: &[Fn], j: usize| symbol(pkg_of(fns[j].file), &fns[j].name);
    // References: a topology for the whole input, then some noise.
    let topology = u.int_in_range(0..=6u8)?;
    let mut targets: Vec<(usize, usize)> = vec![];
    for j in 0..nfns {
        match topology {
            0 => targets.push((j, (j + 1) % nfns.max(1))), // ring (a chain closed on itself)
            1 if j + 1 < nfns => targets.push((j, j + 1)), // long chain
            2 => {
                // star: everything calls f0 and f0 calls everything
                targets.push((j, 0));
                targets.push((0, j));
            }
            3 => targets.push((j, j)), // self-loops
            4 => {
                for k in 0..nfns.min(12) {
                    targets.push((j, k)); // dense
                }
            }
            _ => {}
        }
    }
    for _ in 0..u.int_in_range(0..=60usize)? {
        if nfns > 0 {
            targets.push((u.int_in_range(0..=nfns - 1)?, u.int_in_range(0..=nfns - 1)?));
        }
    }
    for (from, to) in targets {
        let sym = own_sym(&fns, to);
        let name = fns[to].name.clone();
        fns[from].calls.push((name, sym));
    }
    for j in 0..nfns {
        if u.ratio(1u8, 6)? {
            let sym = symbol("tokio", "spawn");
            fns[j].calls.push(("spawn".to_string(), sym));
        }
        if u.ratio(1u8, 8)? {
            let sym = symbol(pkg_of(fns[j].file), "ghost");
            fns[j].calls.push(("ghost".to_string(), sym));
        }
    }

    let mut docs = vec![];
    for (fi, (path, _)) in paths.iter().enumerate() {
        let mut text = String::new();
        let mut occs: Vec<Occurrence> = vec![];
        let mut add = |text: &str, at: usize, len: usize, symbol: String, def: bool| {
            let line = text[..at].matches('\n').count();
            let col = at - text[..at].rfind('\n').map_or(0, |i| i + 1);
            occs.push(Occurrence {
                range: RawRange {
                    start_line: line as u32,
                    start_col: col as u32,
                    end_line: line as u32,
                    end_col: (col + len) as u32,
                },
                symbol,
                roles: i32::from(def),
            });
        };
        // Code outside every declaration that refers to something: a file-level edge.
        if u.ratio(1u8, 3)? && nfns > 0 {
            let to = u.int_in_range(0..=nfns - 1)?;
            text.push_str("use ");
            let at = text.len();
            text.push_str(&fns[to].name);
            add(&text, at, fns[to].name.len(), own_sym(&fns, to), false);
            text.push_str(";\n");
        }
        for (j, f) in fns.iter().enumerate().filter(|(_, f)| f.file == fi) {
            text.push_str("fn ");
            let at = text.len();
            text.push_str(&f.name);
            add(&text, at, f.name.len(), own_sym(&fns, j), true);
            text.push_str("() {\n");
            for (name, sym) in &f.calls {
                text.push_str("    ");
                let at = text.len();
                text.push_str(name);
                add(&text, at, name.len(), sym.clone(), false);
                text.push_str("();\n");
            }
            text.push_str("}\n");
        }
        let full = r.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, &text).unwrap();
        docs.push(Document {
            path: path.clone(),
            encoding: Encoding::Utf8,
            occurrences: occs,
        });
    }
    // Documents the index lists but the workspace lacks, with awkward names.
    for p in ["gone.rs", "weird\nname.rs", "a</script>.rs", "\u{202e}x.rs"] {
        if u.ratio(1u8, 5)? {
            docs.push(Document {
                path: p.to_string(),
                encoding: Encoding::Utf8,
                occurrences: vec![],
            });
        }
    }
    Ok(Fixture {
        index: ScipIndex::new(docs, HashMap::new()),
    })
}

fn provenance(u: &mut Unstructured<'_>) -> Result<Provenance> {
    let text = |u: &mut Unstructured<'_>| -> Result<String> {
        Ok(match u.int_in_range(0..=3u8)? {
            0 => u.arbitrary::<String>()?,
            1 => "</script><img src=x onerror=1>\n'\"&".to_string(),
            2 => "feature/x".to_string(),
            _ => "abc1234".to_string(),
        })
    };
    let prov = Provenance {
        repo: [
            "github:o/n",
            "local:/work/repo",
            "bitbucket:w/n",
            "nonsense",
        ][u.choose_index(4)?]
        .to_string(),
        worktree: text(u)?,
        branch: if u.arbitrary()? { Some(text(u)?) } else { None },
        commit: if u.arbitrary()? { Some(text(u)?) } else { None },
        dirty: u.arbitrary()?,
        shared: false,
    };
    Ok(if u.ratio(1u8, 4)? {
        prov.for_sharing()
    } else {
        prov
    })
}

fn view_spec(u: &mut Unstructured<'_>, prov: Provenance) -> Result<ViewSpec> {
    let mut s = ViewSpec::new(prov);
    s.direction = [Direction::In, Direction::Out, Direction::Both][u.choose_index(3)?];
    s.depth = match u.int_in_range(0..=9u8)? {
        0 => 0,
        1 => 1000,
        _ => u.int_in_range(1..=6usize)?,
    };
    if u.ratio(1u8, 3)? {
        for k in EdgeKind::ALL {
            if u.arbitrary()? {
                s.kinds.push(k);
            }
        }
    }
    s.max_nodes = match u.int_in_range(0..=5u8)? {
        0 => 0,
        1 => 1,
        2 => 3,
        3 => 60,
        _ => u.int_in_range(0..=100usize)?,
    };
    s.externals = u.arbitrary()?;
    s.file_level = u.arbitrary()?;
    Ok(s)
}

/// Splits `cmd` like a POSIX shell for the quoting `cho open '…'` uses: unquoted words,
/// single quotes, and backslash outside quotes.
fn shell_words(cmd: &str) -> Vec<String> {
    let mut words = vec![];
    let mut cur = String::new();
    let mut started = false;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' => {
                if started {
                    words.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '\\' => {
                started = true;
                cur.extend(chars.next());
            }
            c => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        words.push(cur);
    }
    words
}

// ---- checks -----------------------------------------------------------------------

/// Replaces each quoted string of a DOT line with `S`; `None` when a string is unterminated.
fn normalise(line: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            out.push(c);
            continue;
        }
        out.push('S');
        loop {
            match chars.next()? {
                '\\' => {
                    chars.next()?;
                }
                '"' => break,
                _ => {}
            }
        }
    }
    Some(out)
}

fn allowed_dot_lines() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        let mut s: HashSet<String> = [
            "digraph cho {",
            "rankdir=LR;",
            "compound=true;",
            "node [shape=box, fontname=S, fontsize=10];",
            "edge [fontname=S, fontsize=8];",
            "subgraph S {",
            "label=S;",
            "}",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        for shape in ["box", "ellipse", "component"] {
            for focus in ["", ", style=filled, fillcolor=lightyellow"] {
                for url in ["", ", URL=S"] {
                    s.insert(format!("S [label=S, shape={shape}{focus}{url}];"));
                }
            }
        }
        for style in ["solid", "dashed", "dotted"] {
            for color in ["", ", color=red"] {
                s.insert(format!("S -> S [label=S, style={style}{color}];"));
            }
        }
        s
    })
}

fn check_dot(g: &Graph) {
    let dot = to_dot(g);
    let mut comments = 0;
    for line in dot.lines() {
        if line.starts_with("//") {
            comments += 1;
            assert!(
                !line.contains('\r'),
                "comment with a carriage return: {line:?}"
            );
            continue;
        }
        let norm = normalise(line).unwrap_or_else(|| panic!("unterminated string in {line:?}"));
        assert!(
            allowed_dot_lines().contains(norm.trim()),
            "unexpected DOT line {line:?} ({norm:?})"
        );
    }
    assert_eq!(
        comments, 2,
        "DOT comments: text escaped its comment line:\n{dot}"
    );
}

/// The tags `to_html` writes itself; node text must never add to them.
fn tag_ok(tag: &str) -> bool {
    const FIXED: [&str; 24] = [
        "<!doctype html>",
        "<head>",
        "</head>",
        "<body>",
        "</body>",
        "</html>",
        "<header>",
        "</header>",
        "<main>",
        "</main>",
        "<title>",
        "</title>",
        "<h1>",
        "</h1>",
        "<span>",
        "</span>",
        "<b>",
        "</b>",
        "<b class=\"dirty\">",
        "<ul class=\"notes\">",
        "</ul>",
        "<li>",
        "</li>",
        "</div>",
    ];
    FIXED.contains(&tag)
        || tag.starts_with("<html lang=\"en\"")
        || tag == "<meta charset=\"utf-8\">"
        || tag == "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
        || matches!(
            tag,
            "<div class=\"meta\">"
                | "<div class=\"limits\">"
                | "<div class=\"bar\" id=\"bar\">"
                | "<div id=\"cy\">"
                | "<aside id=\"side\">"
                | "</aside>"
        )
}

fn check_html(g: &Graph) {
    let page = to_html(
        g,
        &PageOptions {
            theme: None,
            assets: Assets::Shared("../assets/".into()),
        },
    );
    // The data block holds JSON with no `<` that reads back as the graph.
    let open = "<script id=\"cho-data\" type=\"application/json\">";
    let start = page.find(open).expect("data block") + open.len();
    let end = start + page[start..].find("</script").expect("data block end");
    let data = &page[start..end];
    assert!(!data.contains('<'), "raw `<` in the data block");
    let back: serde_json::Value = serde_json::from_str(data).expect("data block is JSON");
    assert_eq!(
        back,
        serde_json::from_str::<serde_json::Value>(&g.to_json()).unwrap()
    );

    // Everything outside style, scripts and comments is the template's markup plus escaped text.
    let mut rest = String::new();
    let mut s = page.as_str();
    while !s.is_empty() {
        let next = ["<style>", "<script", "<!--"]
            .iter()
            .filter_map(|m| s.find(m).map(|i| (i, *m)))
            .min();
        let Some((i, marker)) = next else {
            rest.push_str(s);
            break;
        };
        rest.push_str(&s[..i]);
        let close = match marker {
            "<style>" => "</style>",
            "<script" => "</script>",
            _ => "-->",
        };
        let after = &s[i..];
        let Some(j) = after.find(close) else {
            panic!("unterminated {marker}");
        };
        s = &after[j + close.len()..];
    }
    let mut tags = rest.split('<');
    let text0 = tags.next().unwrap();
    assert!(!text0.contains('>'));
    for t in tags {
        let (tag, text) = t.split_once('>').expect("tag closes");
        assert!(tag_ok(&format!("<{tag}>")), "unexpected markup <{tag}>");
        assert!(
            !text.contains('"') && !text.contains('>'),
            "unescaped text after <{tag}>: {text:?}"
        );
    }
}

thread_local! {
    /// Off for the second, repeat run, which only compares output.
    static CHECKS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    /// The page is checked once per input: building it parses the theme tables each time.
    static PAGE_CHECKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn check_graph(
    gr: &Graph,
    g: &RefGraph,
    spec: &ViewSpec,
    dir: Direction,
    depth: usize,
    forced_externals: bool,
) {
    if !CHECKS.get() {
        return;
    }
    let ids: HashSet<&str> = gr.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids.len(), gr.nodes.len(), "duplicate node ids");
    for n in &gr.nodes {
        if let Some(p) = &n.parent {
            assert!(
                ids.contains(p.as_str()),
                "parent {p} of {} is not a node",
                n.id
            );
        }
        if let Some(d) = n.depth {
            assert!(d <= depth, "{} at depth {d} beyond {depth}", n.id);
        }
        if n.focus {
            assert_eq!(n.depth, Some(0));
        }
        if n.kind == NodeKind::External && !(spec.externals || forced_externals) {
            panic!("external node {} in a view without externals", n.id);
        }
        if n.kind == NodeKind::More {
            assert!(n.hidden.is_some_and(|h| h > 0));
            assert!(gr.view.truncated, "`more` node without truncation");
        }
        if let (Some(link), Some(cmd)) = (&n.link, &n.open_cmd) {
            assert!(
                !link.contains(|c: char| c.is_whitespace() || c.is_control()),
                "link has whitespace or control characters: {link:?}"
            );
            // One single-quoted shell word: `'` closes the quote, is escaped, and reopens it.
            assert_eq!(*cmd, format!("cho open '{}'", link.replace('\'', "'\\''")));
            assert!(shell_words(cmd) == vec!["cho".to_string(), "open".to_string(), link.clone()]);
        }
    }
    let by_id: HashMap<&str, &cho_graph::Node> =
        gr.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let focus = gr.nodes.iter().filter(|n| n.focus).count();
    let symbols = gr
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Symbol)
        .count();
    assert!(
        symbols <= spec.max_nodes.max(focus),
        "{symbols} symbol nodes over the cap {} (focus {focus})",
        spec.max_nodes
    );

    let by_symbol: HashMap<String, u32> = g
        .decls
        .iter()
        .enumerate()
        .map(|(i, d)| (format!("sym:{}", d.symbol.symbol), i as u32))
        .collect();
    let depth_of = |id: &str| by_id.get(id).and_then(|n| n.depth);
    let mut seen_edges = HashSet::new();
    for e in &gr.edges {
        assert!(
            ids.contains(e.from.as_str()),
            "edge from {} which is not a node",
            e.from
        );
        assert!(
            ids.contains(e.to.as_str()),
            "edge to {} which is not a node",
            e.to
        );
        assert!(e.count > 0 && e.mapped <= e.count);
        assert!(
            spec.kinds.is_empty() || spec.kinds.contains(&e.kind),
            "edge of kind {:?} not asked for",
            e.kind
        );
        assert!(
            seen_edges.insert((&e.from, &e.to, e.kind)),
            "duplicate edge"
        );
        // A decl-to-decl edge is one the graph has, with the same count.
        if let (Some(&a), Some(&b)) = (by_symbol.get(&e.from), by_symbol.get(&e.to)) {
            let found = g.out_edges(cho_graph::NodeRef::Decl(a)).any(|r| {
                r.to == cho_graph::NodeRef::Decl(b) && r.kind == e.kind && r.count == e.count
            });
            assert!(found, "edge {} -> {} is not in the graph", e.from, e.to);
        }
        // Nodes sit at their shortest distance from the start set.
        let (df, dt) = (depth_of(&e.from), depth_of(&e.to));
        if let (Some(df), Some(dt)) = (df, dt) {
            let expands = e.from.starts_with("sym:") || df == 0;
            let file_level_ref = e.to.starts_with("filelevel:")
                && e.kind == EdgeKind::References
                && !spec.file_level;
            if dir != Direction::In && expands && !file_level_ref {
                assert!(dt <= df + 1, "{} -> {}: depth {df} then {dt}", e.from, e.to);
            }
            let expands = e.to.starts_with("sym:") || dt == 0;
            let file_level_ref = e.from.starts_with("filelevel:")
                && e.kind == EdgeKind::References
                && !spec.file_level;
            if dir != Direction::Out && expands && !file_level_ref {
                assert!(df <= dt + 1, "{} -> {}: depth {df} then {dt}", e.from, e.to);
            }
        }
    }
    // Every node is reachable from the start set within the depth.
    for n in &gr.nodes {
        let Some(d) = n.depth else { continue };
        if d == 0 {
            assert!(n.focus, "{} at depth 0 but not in the start set", n.id);
            continue;
        }
        let ok = gr.edges.iter().any(|e| {
            (dir != Direction::In && e.to == n.id && depth_of(&e.from) == Some(d - 1))
                || (dir != Direction::Out && e.from == n.id && depth_of(&e.to) == Some(d - 1))
        });
        assert!(ok, "{} at depth {d} has no edge to depth {}", n.id, d - 1);
    }
    // Cycle flags agree with plain reachability.
    if gr.edges.len() <= 400 {
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &gr.edges {
            adj.entry(&e.from).or_default().push(&e.to);
        }
        let reaches = |from: &str, to: &str| -> bool {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut stack = vec![from];
            while let Some(x) = stack.pop() {
                if x == to {
                    return true;
                }
                if seen.insert(x) {
                    stack.extend(adj.get(x).into_iter().flatten().copied());
                }
            }
            false
        };
        for e in &gr.edges {
            let expect = e.from != e.to && reaches(&e.to, &e.from);
            assert_eq!(e.in_cycle, expect, "cycle flag of {} -> {}", e.from, e.to);
        }
    }
    for r in gr.rollups.file.iter().chain(&gr.rollups.package) {
        assert!(ids.contains(r.from.as_str()) && ids.contains(r.to.as_str()));
        assert_ne!(r.from, r.to);
        assert!(r.symbols <= r.references);
    }
    check_dot(gr);
    if !PAGE_CHECKED.replace(true) {
        check_html(gr);
    }
}

fn check_changed(g: &RefGraph, hunks: &[Hunk], c: &Changed) {
    let overlap = |h: &Hunk| -> Vec<u32> {
        let Some(f) = g.files.iter().position(|f| f.path == h.path) else {
            return vec![];
        };
        g.decls
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.file == f as u32 && d.start_line <= h.end_line && h.start_line <= d.end_line
            })
            .map(|(i, _)| i as u32)
            .collect()
    };
    let mut union: BTreeSet<u32> = BTreeSet::new();
    let mut file_level: BTreeSet<u32> = BTreeSet::new();
    let mut unindexed: BTreeSet<String> = BTreeSet::new();
    for h in hunks {
        match g.files.iter().position(|f| f.path == h.path) {
            None => {
                unindexed.insert(h.path.clone());
            }
            Some(f) => {
                let o = overlap(h);
                if o.is_empty() {
                    file_level.insert(f as u32);
                } else {
                    assert!(
                        o.iter().any(|d| c.decls.contains(d)),
                        "hunk {h:?} touches declarations but none is reported"
                    );
                    union.extend(o);
                }
            }
        }
    }
    assert!(
        c.decls.iter().all(|d| union.contains(d)),
        "declaration not under a hunk"
    );
    assert_eq!(
        c.file_level.iter().copied().collect::<BTreeSet<_>>(),
        file_level
    );
    assert_eq!(
        c.unindexed.iter().cloned().collect::<BTreeSet<_>>(),
        unindexed
    );
    // No reported declaration contains another reported declaration of the same file.
    for &a in &c.decls {
        for &b in &c.decls {
            let (da, db) = (g.decl(a), g.decl(b));
            let strictly = da.file == db.file
                && a != b
                && da.start_line <= db.start_line
                && db.end_line <= da.end_line
                && (da.start_line, da.end_line) != (db.start_line, db.end_line);
            assert!(!strictly, "outer declaration {a} reported with inner {b}");
        }
    }
}

fn check_resolve(g: &RefGraph, u: &mut Unstructured<'_>) -> Result<()> {
    for (i, d) in g.decls.iter().enumerate() {
        assert_eq!(resolve_symbol(g, &d.symbol.symbol), Ok(i as u32));
        match resolve_symbol(g, &d.qualified) {
            Ok(j) => assert!(g.decl(j).qualified == d.qualified || g.decl(j).name == d.qualified),
            Err(ResolveError::Ambiguous { candidates, .. }) => assert!(candidates.len() > 1),
            Err(ResolveError::NotFound(q)) => panic!("`{q}` is a declaration but was not found"),
        }
        let pkg = g.package_name(d.package);
        let q = format!("{pkg}:{}", d.name);
        match resolve_symbol(g, &q) {
            Ok(j) => assert_eq!(g.decl(j).name, d.name),
            Err(ResolveError::Ambiguous { candidates, .. }) => assert!(candidates.len() > 1),
            Err(ResolveError::NotFound(_)) => {
                // Only a package or file name with a `:` or `#` in it can confuse the split.
                assert!(pkg.contains([':', '#']) || g.file(d.file).path.contains(['#', ':']))
            }
        }
    }
    let q: String = u.arbitrary()?;
    if let Ok(i) = resolve_symbol(g, &q) {
        assert!((i as usize) < g.decls.len());
    }
    Ok(())
}

fn run_views(g: &RefGraph, u: &mut Unstructured<'_>, prov: &Provenance) -> Result<Vec<String>> {
    let mut outputs = vec![];
    let n = g.decls.len() as u32;
    for _ in 0..u.int_in_range(1..=4)? {
        let spec = view_spec(u, prov.clone())?;
        let starts: Vec<u32> = if n == 0 {
            vec![]
        } else {
            (0..u.int_in_range(1..=3usize)?)
                .map(|_| u.int_in_range(0..=n - 1))
                .collect::<Result<_>>()?
        };
        if !starts.is_empty() || n == 0 {
            let gr = neighbourhood(g, &starts, &spec);
            check_graph(&gr, g, &spec, spec.direction, spec.depth, false);
            outputs.push(gr.to_json());
            let gr = outbound_closure(g, &starts, &spec);
            check_graph(&gr, g, &spec, Direction::Out, spec.depth, true);
            outputs.push(gr.to_json());
        }
        if !g.files.is_empty() {
            let f = &g.files[u.int_in_range(0..=g.files.len() - 1)?].path;
            let gr = file_view(g, f, &spec).expect("an indexed file has a view");
            check_graph(&gr, g, &spec, spec.direction, spec.depth, true);
            outputs.push(gr.to_json());
        }
        assert!(file_view(g, "no/such/file.rs", &spec).is_none());
        if !g.packages.is_empty() {
            let p = &g.packages[u.int_in_range(0..=g.packages.len() - 1)?].name;
            let gr = package_view(g, p, &spec).expect("a package has a view");
            check_graph(&gr, g, &spec, spec.direction, spec.depth.min(1), true);
            outputs.push(gr.to_json());
        }
        let mut hunks = vec![];
        for _ in 0..u.int_in_range(0..=4)? {
            let path = if g.files.is_empty() || u.ratio(1u8, 6)? {
                "ghost.rs".to_string()
            } else {
                g.files[u.int_in_range(0..=g.files.len() - 1)?].path.clone()
            };
            let start = u.int_in_range(1..=60usize)?;
            hunks.push(Hunk {
                path,
                start_line: start,
                end_line: start + u.int_in_range(0..=10usize)?,
            });
        }
        let changed = changed_decls(g, &hunks);
        if CHECKS.get() {
            check_changed(g, &hunks, &changed);
        }
        let gr = blast_radius(g, &changed, &[], &spec, vec![]);
        check_graph(&gr, g, &spec, Direction::In, spec.depth, false);
        outputs.push(gr.to_json());
    }
    Ok(outputs)
}

fn structured(data: &[u8]) -> Result<()> {
    PAGE_CHECKED.set(false);
    let mut u = Unstructured::new(data);
    let fx = fixture(&mut u)?;
    let prov = provenance(&mut u)?;
    let sem = Semantic::new(fx.index, root(), IndexStatus::Current);
    let g = RefGraph::build(&sem);

    // The raw graph: valid indices, no self-loops, no duplicate edges.
    let mut seen = HashSet::new();
    for e in &g.edges {
        assert!(e.count > 0 && e.mapped <= e.count);
        assert_ne!(e.from, e.to);
        assert!(seen.insert((e.from, e.to, e.kind)));
        for n in [e.from, e.to] {
            match n {
                cho_graph::NodeRef::Decl(i) => assert!((i as usize) < g.decls.len()),
                cho_graph::NodeRef::File(i) => assert!((i as usize) < g.files.len()),
                cho_graph::NodeRef::External(i) => assert!((i as usize) < g.externals.len()),
            }
        }
    }
    let mut symbols = HashSet::new();
    assert!(
        g.decls.iter().all(|d| symbols.insert(&d.symbol)),
        "two declarations share a symbol"
    );
    assert!(
        g.packages.iter().all(|p| p.name != "leaked-outside"),
        "a manifest outside the repository named a package"
    );
    let rest = u.take_rest();
    check_resolve(&g, &mut Unstructured::new(rest))?;

    // Same input, same output, also from a graph built again.
    let first = run_views(&g, &mut Unstructured::new(rest), &prov)?;
    let g2 = RefGraph::build(&sem);
    CHECKS.set(false);
    let second = run_views(&g2, &mut Unstructured::new(rest), &prov);
    CHECKS.set(true);
    let second = second?;
    assert_eq!(
        first, second,
        "views differ between two builds of one index"
    );
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = structured(data);
});
