//! Views over a [`RefGraph`]: neighbourhood, file or package, outbound closure and
//! blast radius. A view walks edges, applies the node cap, and assembles a [`Graph`]
//! with containment (symbol, file, package), roll-ups, links and cycle flags.

use crate::build::{NodeRef, RefGraph};
use crate::model::*;
use crate::packages::same_package;
use cho_core::{ChorongLink, RepoRef, web_url};
use cho_semantic::IndexStatus;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    In,
    Out,
    Both,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
            Self::Both => "both",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ViewSpec {
    pub direction: Direction,
    /// Hops from the starting set.
    pub depth: usize,
    /// Only these edge kinds; empty means all.
    pub kinds: Vec<EdgeKind>,
    /// Declarations kept before the rest become "+N more" nodes.
    pub max_nodes: usize,
    /// Show third-party packages (one node each).
    pub externals: bool,
    /// Draw `references` from code outside any declaration (imports, impl headers) as
    /// nodes of their own. Off by default: they restate the symbol-level edges.
    pub file_level: bool,
    pub provenance: Provenance,
}

impl ViewSpec {
    pub fn new(provenance: Provenance) -> Self {
        Self {
            direction: Direction::Both,
            depth: 2,
            kinds: vec![],
            max_nodes: 60,
            externals: false,
            file_level: false,
            provenance,
        }
    }

    fn allows(&self, k: EdgeKind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&k)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    NotFound(String),
    Ambiguous {
        query: String,
        candidates: Vec<String>,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(q) => write!(f, "no declaration matches `{q}`"),
            Self::Ambiguous { query, candidates } => write!(
                f,
                "`{query}` is ambiguous; use PACKAGE:NAME or FILE#NAME. Candidates:\n  {}",
                candidates.join("\n  ")
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Finds one declaration. `query` is an exact SCIP symbol, `NAME`, `QUALIFIED::NAME`,
/// `PACKAGE:NAME` or `FILE#NAME`. Several matches are an error that lists them; the
/// engine never picks one.
pub fn resolve_symbol(g: &RefGraph, query: &str) -> Result<u32, ResolveError> {
    if let Some(i) = g.decls.iter().position(|d| d.symbol.symbol == query) {
        return Ok(i as u32);
    }
    let (pkg, file, name) = split_query(g, query);
    let hits: Vec<u32> = g
        .decls
        .iter()
        .enumerate()
        .filter(|(_, d)| {
            let name_ok = d.qualified == name
                || d.name == name
                || d.qualified.ends_with(&format!("::{name}"))
                || d.qualified.ends_with(&format!(".{name}"));
            let pkg_ok = pkg.is_none_or(|p| same_package(g.package_name(d.package), p));
            let file_ok = file.is_none_or(|f| g.file(d.file).path == f);
            name_ok && pkg_ok && file_ok
        })
        .map(|(i, _)| i as u32)
        .collect();
    match hits.as_slice() {
        [] => Err(ResolveError::NotFound(query.to_string())),
        [one] => Ok(*one),
        many => Err(ResolveError::Ambiguous {
            query: query.to_string(),
            candidates: many
                .iter()
                .map(|&i| {
                    let d = g.decl(i);
                    format!(
                        "{}:{} ({}, {}:{})",
                        g.package_name(d.package),
                        d.qualified,
                        d.kind.as_str(),
                        g.file(d.file).path,
                        d.line
                    )
                })
                .collect(),
        }),
    }
}

fn split_query<'q>(g: &RefGraph, q: &'q str) -> (Option<&'q str>, Option<&'q str>, &'q str) {
    if let Some((f, n)) = q.rsplit_once('#')
        && !f.contains(' ')
        && g.files.iter().any(|x| x.path == f)
    {
        return (None, Some(f), n);
    }
    // First `:` that is not part of `::`.
    let b = q.as_bytes();
    for i in 0..b.len() {
        if b[i] == b':' && b.get(i + 1) != Some(&b':') && (i == 0 || b[i - 1] != b':') {
            return (Some(&q[..i]), None, &q[i + 1..]);
        }
    }
    (None, None, q)
}

/// A hunk on the head side of a diff: one-based, inclusive lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// What a diff touches in the indexed code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changed {
    pub decls: Vec<u32>,
    /// Files with changes outside every declaration.
    pub file_level: Vec<u32>,
    /// Paths with hunks that the index has no document for.
    pub unindexed: Vec<String>,
}

/// Diff hunks intersected with declarations in the head: the innermost declaration
/// overlapping each hunk.
pub fn changed_decls(g: &RefGraph, hunks: &[Hunk]) -> Changed {
    let mut out = Changed::default();
    let mut decls = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut unindexed = BTreeSet::new();
    for h in hunks {
        let Some(f) = g.files.iter().position(|f| f.path == h.path) else {
            unindexed.insert(h.path.clone());
            continue;
        };
        let overlapping: Vec<u32> = g
            .decls
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.file == f as u32 && d.start_line <= h.end_line && h.start_line <= d.end_line
            })
            .map(|(i, _)| i as u32)
            .collect();
        let inner: Vec<u32> = overlapping
            .iter()
            .copied()
            .filter(|&a| {
                let da = g.decl(a);
                !overlapping.iter().any(|&b| {
                    let db = g.decl(b);
                    b != a
                        && da.start_line <= db.start_line
                        && db.end_line <= da.end_line
                        && (da.start_line, da.end_line) != (db.start_line, db.end_line)
                })
            })
            .collect();
        if inner.is_empty() {
            files.insert(f as u32);
        }
        decls.extend(inner);
    }
    out.decls = decls.into_iter().collect();
    out.file_level = files.into_iter().collect();
    out.unindexed = unindexed.into_iter().collect();
    out
}

pub fn neighbourhood(g: &RefGraph, start: &[u32], spec: &ViewSpec) -> Graph {
    let starts: Vec<NodeRef> = start.iter().map(|&i| NodeRef::Decl(i)).collect();
    let title = format!("Neighbourhood of {}", names(g, &starts));
    let w = walk(g, &starts, spec.direction, spec.depth, spec);
    assemble(
        g,
        &w,
        spec,
        ViewKind::Neighbourhood,
        title,
        spec.direction,
        vec![],
    )
}

/// Everything `start` depends on, to the full depth of `spec`, drawn at package level.
pub fn outbound_closure(g: &RefGraph, start: &[u32], spec: &ViewSpec) -> Graph {
    let mut spec = spec.clone();
    spec.externals = true;
    let starts: Vec<NodeRef> = start.iter().map(|&i| NodeRef::Decl(i)).collect();
    let title = format!("Outbound closure of {}", names(g, &starts));
    let w = walk(g, &starts, Direction::Out, spec.depth, &spec);
    assemble(
        g,
        &w,
        &spec,
        ViewKind::OutboundClosure,
        title,
        Direction::Out,
        vec![],
    )
}

/// A file's declarations and everything one hop around them (`spec.depth`), at package level.
pub fn file_view(g: &RefGraph, path: &str, spec: &ViewSpec) -> Option<Graph> {
    let f = g.files.iter().position(|f| f.path == path)? as u32;
    let mut starts: Vec<NodeRef> = g
        .decls
        .iter()
        .enumerate()
        .filter(|(_, d)| d.file == f)
        .map(|(i, _)| NodeRef::Decl(i as u32))
        .collect();
    starts.push(NodeRef::File(f));
    let mut spec = spec.clone();
    spec.externals = true;
    let w = walk(g, &starts, spec.direction, spec.depth, &spec);
    let title = format!("File {path}");
    Some(assemble(
        g,
        &w,
        &spec,
        ViewKind::File,
        title,
        spec.direction,
        vec![],
    ))
}

/// A package's declarations that cross its boundary, and the other side, at package level.
pub fn package_view(g: &RefGraph, package: &str, spec: &ViewSpec) -> Option<Graph> {
    let p = g
        .packages
        .iter()
        .position(|p| same_package(&p.name, package))? as u32;
    let mut starts: Vec<(u64, NodeRef)> = vec![];
    for (i, d) in g.decls.iter().enumerate() {
        if d.package != p {
            continue;
        }
        let n = NodeRef::Decl(i as u32);
        let crossing: u64 = g
            .out_edges(n)
            .map(|e| (e.to, e.count))
            .chain(g.in_edges(n).map(|e| (e.from, e.count)))
            .filter(|(o, _)| g.package_of(*o) != Some(p))
            .map(|(_, c)| c as u64)
            .sum();
        if crossing > 0 {
            starts.push((crossing, n));
        }
    }
    starts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let total = starts.len();
    let keep: Vec<NodeRef> = starts
        .into_iter()
        .take(spec.max_nodes)
        .map(|(_, n)| n)
        .collect();
    let mut spec = spec.clone();
    spec.externals = true;
    spec.depth = spec.depth.min(1);
    let mut w = walk(g, &keep, spec.direction, spec.depth, &spec);
    let mut notes = vec![];
    if total > keep.len() {
        notes.push(format!(
            "{} of {} boundary declarations of {} are shown; the rest are not drawn.",
            keep.len(),
            total,
            g.packages[p as usize].name
        ));
        w.truncated = true;
    }
    let title = format!("Package {}", g.packages[p as usize].name);
    Some(assemble(
        g,
        &w,
        &spec,
        ViewKind::Package,
        title,
        spec.direction,
        notes,
    ))
}

/// Who could be affected by a change: inbound edges from the changed declarations,
/// with the test files that reference them marked (reachability, not coverage).
/// `removed` are the paths the diff deletes; `notes` are what only the caller knows (a dirty
/// checkout, a pull request), shown first.
pub fn blast_radius(
    g: &RefGraph,
    changed: &Changed,
    removed: &[String],
    spec: &ViewSpec,
    notes: Vec<String>,
) -> Graph {
    let mut starts: Vec<NodeRef> = changed.decls.iter().map(|&i| NodeRef::Decl(i)).collect();
    starts.extend(changed.file_level.iter().map(|&f| NodeRef::File(f)));
    let w = walk(g, &starts, Direction::In, spec.depth, spec);
    let mut notes = notes;
    if !removed.is_empty() {
        notes.push(format!(
            "Deleted in the diff (their edges are not in a head-only index): {}.",
            removed.join(", ")
        ));
    }
    if starts.is_empty() {
        notes.push("No indexed declaration overlaps the diff.".into());
    }
    if !changed.file_level.is_empty() {
        notes.push(format!(
            "{} changed file(s) have edits outside any declaration; shown as file-level code.",
            changed.file_level.len()
        ));
    }
    if !changed.unindexed.is_empty() {
        notes.push(format!(
            "Changed but not in the index (no edges): {}.",
            changed.unindexed.join(", ")
        ));
    }
    notes.push(
        "Indexing only the head shows who uses the new code; removed exports and removed edges need the merge base indexed too.".into(),
    );
    notes.push(
        "Marked tests reference the changed code (by path and name convention); that is reachability, not coverage.".into(),
    );
    let title = format!("Blast radius of {}", names(g, &starts));
    let mut graph = assemble(
        g,
        &w,
        spec,
        ViewKind::BlastRadius,
        title,
        Direction::In,
        notes,
    );
    let mut tests: BTreeSet<String> = BTreeSet::new();
    for n in &graph.nodes {
        if n.kind == NodeKind::File && n.is_test {
            tests.insert(n.path.clone().unwrap_or_default());
        }
    }
    graph.reachable_tests = tests.into_iter().collect();
    graph
}

fn names(g: &RefGraph, starts: &[NodeRef]) -> String {
    let mut v: Vec<String> = starts
        .iter()
        .take(3)
        .map(|n| match *n {
            NodeRef::Decl(i) => g.decl(i).qualified.clone(),
            NodeRef::File(f) => g.file(f).path.clone(),
            NodeRef::External(e) => g.externals[e as usize].clone(),
        })
        .collect();
    if starts.len() > 3 {
        v.push(format!("and {} more", starts.len() - 3));
    }
    v.join(", ")
}

// ---- walking ----------------------------------------------------------------------

/// An edge toward the frontier: (node, kind, count, mapped count, outgoing).
type Via = (NodeRef, EdgeKind, u32, u32, bool);
type RollAcc<'a> = (u32, BTreeSet<&'a str>, BTreeMap<String, u32>);

struct Walk {
    starts: Vec<NodeRef>,
    visited: BTreeMap<NodeRef, usize>,
    /// Dropped declarations per package.
    overflow: BTreeMap<u32, BTreeSet<NodeRef>>,
    /// Edges from a kept node to dropped declarations of a package.
    more_edges: BTreeMap<(NodeRef, u32, EdgeKind, bool), (u32, u32)>,
    truncated: bool,
    /// File-level `references` left out (see `ViewSpec::file_level`).
    omitted_file_level: u32,
}

fn walk(g: &RefGraph, starts: &[NodeRef], dir: Direction, depth: usize, spec: &ViewSpec) -> Walk {
    let mut w = Walk {
        starts: starts.to_vec(),
        visited: BTreeMap::new(),
        overflow: BTreeMap::new(),
        more_edges: BTreeMap::new(),
        truncated: false,
        omitted_file_level: 0,
    };
    for &s in starts {
        w.visited.insert(s, 0);
    }
    let mut budget = spec.max_nodes.saturating_sub(
        w.visited
            .keys()
            .filter(|n| !matches!(n, NodeRef::External(_)))
            .count(),
    );
    let mut frontier: Vec<NodeRef> = starts.to_vec();
    for d in 0..depth {
        if frontier.is_empty() {
            break;
        }
        // Candidate -> (weight, edges toward the frontier).
        let mut cand: BTreeMap<NodeRef, (u64, Vec<Via>)> = BTreeMap::new();
        for &u in &frontier {
            let mut visit = |e: &crate::build::RawEdge, outgoing: bool| {
                if !spec.allows(e.kind) {
                    return;
                }
                let v = if outgoing { e.to } else { e.from };
                if matches!(v, NodeRef::External(_)) && !spec.externals {
                    return;
                }
                if w.visited.contains_key(&v) {
                    return;
                }
                if matches!(v, NodeRef::File(_))
                    && !spec.file_level
                    && e.kind == EdgeKind::References
                {
                    w.omitted_file_level += e.count;
                    return;
                }
                let c = cand.entry(v).or_default();
                c.0 += e.count as u64;
                c.1.push((u, e.kind, e.count, e.mapped, outgoing));
            };
            if dir != Direction::In {
                g.out_edges(u).for_each(|e| visit(e, true));
            }
            if dir != Direction::Out {
                g.in_edges(u).for_each(|e| visit(e, false));
            }
        }
        let mut ranked: Vec<(NodeRef, u64, Vec<Via>)> =
            cand.into_iter().map(|(n, (wt, es))| (n, wt, es)).collect();
        ranked.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| sort_key(g, a.0).cmp(&sort_key(g, b.0)))
        });
        let mut next = vec![];
        for (n, _, es) in ranked {
            match n {
                NodeRef::External(_) => {
                    w.visited.insert(n, d + 1);
                }
                _ if budget > 0 => {
                    budget -= 1;
                    w.visited.insert(n, d + 1);
                    if matches!(n, NodeRef::Decl(_)) {
                        next.push(n);
                    }
                }
                NodeRef::Decl(_) | NodeRef::File(_) => {
                    w.truncated = true;
                    let pkg = g
                        .package_of(n)
                        .expect("declarations and files have a package");
                    w.overflow.entry(pkg).or_default().insert(n);
                    for (u, kind, count, mapped, outgoing) in es {
                        let c = w.more_edges.entry((u, pkg, kind, outgoing)).or_default();
                        c.0 += count;
                        c.1 += mapped;
                    }
                }
            }
        }
        frontier = next;
    }
    w
}

fn sort_key(g: &RefGraph, n: NodeRef) -> String {
    match n {
        NodeRef::Decl(i) => g.decl(i).symbol.symbol.clone(),
        NodeRef::File(f) => g.file(f).path.clone(),
        NodeRef::External(e) => g.externals[e as usize].clone(),
    }
}

// ---- assembly ---------------------------------------------------------------------

struct Ids {
    file: HashMap<u32, String>,
    package: HashMap<u32, String>,
}

fn id_of(g: &RefGraph, n: NodeRef) -> String {
    match n {
        NodeRef::Decl(i) => format!("sym:{}", g.decl(i).symbol.symbol),
        NodeRef::File(f) => format!("filelevel:{}", g.file(f).path),
        NodeRef::External(e) => format!("ext:{}", g.externals[e as usize]),
    }
}

fn links(
    prov: &Provenance,
    path: &str,
    symbol: Option<&str>,
    line: Option<usize>,
) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(repo) = RepoRef::parse(&prov.repo) else {
        return (None, None, None);
    };
    let link = ChorongLink::Open {
        repo: repo.clone(),
        pr: None,
        rev: prov.commit.clone(),
        base: None,
        side: None,
        path: Some(path.to_string()),
        symbol: symbol.map(str::to_string),
        line: if symbol.is_none() { line } else { None },
        branch: prov.branch.clone(),
        dirty: prov.dirty,
    }
    .to_url();
    // Marks the link as coming from a graph page, so the viewer can offer "Rebuild graph"
    // when the checkout has moved (`docs/public/url-scheme.md`, "Commit mismatch").
    let link = format!("{link}&from=graph");
    // The branch name comes from the repository and may hold a `'`, which the URL leaves as it
    // is: close the quote around it so the command stays one argument.
    let cmd = format!("cho open '{}'", link.replace('\'', "'\\''"));
    let web = prov
        .commit
        .as_deref()
        .and_then(|rev| web_url(&repo, rev, path, line));
    (Some(link), Some(cmd), web)
}

/// The node of repository package `pkg`, added to `nodes` unless it is there; its id.
fn ensure_package(nodes: &mut BTreeMap<String, Node>, g: &RefGraph, pkg: u32) -> String {
    let pname = g.package_name(pkg).to_string();
    let pid = format!("pkg:{pname}");
    nodes.entry(pid.clone()).or_insert_with(|| {
        let mut n = Node::new(pid.clone(), NodeKind::Package, pname.clone());
        n.package = Some(pname);
        n.scip_kind = Some(g.packages[pkg as usize].kind.as_str().to_string());
        n
    });
    pid
}

/// What a declaration or file-level node says about where it is.
struct Place<'a> {
    parent: String,
    path: &'a str,
    package: u32,
    is_test: bool,
    focus: bool,
    depth: usize,
}

/// A symbol node in a file: a declaration (`symbol`, `line`) or file-level code (neither).
fn located_node(
    g: &RefGraph,
    prov: &Provenance,
    nref: NodeRef,
    label: String,
    scip_kind: String,
    decl: Option<(&str, usize)>,
    at: Place<'_>,
) -> Node {
    let (link, cmd, web) = match decl {
        Some((qualified, line)) => links(prov, at.path, Some(qualified), Some(line)),
        None => links(prov, at.path, None, None),
    };
    let mut n = Node::new(id_of(g, nref), NodeKind::Symbol, label);
    n.parent = Some(at.parent);
    if let NodeRef::Decl(i) = nref {
        n.symbol = Some(g.decl(i).symbol.symbol.clone());
        n.line = decl.map(|(_, line)| line);
    }
    n.scip_kind = Some(scip_kind);
    n.path = Some(at.path.to_string());
    n.package = Some(g.package_name(at.package).to_string());
    n.is_test = at.is_test;
    n.link = link;
    n.open_cmd = cmd;
    n.web_url = web;
    n.focus = at.focus;
    n.depth = Some(at.depth);
    n.unresolved_calls = capped(g.unresolved_calls(nref));
    n
}

fn assemble(
    g: &RefGraph,
    w: &Walk,
    spec: &ViewSpec,
    kind: ViewKind,
    title: String,
    direction: Direction,
    mut notes: Vec<String>,
) -> Graph {
    let prov = &spec.provenance;
    let mut ids = Ids {
        file: HashMap::new(),
        package: HashMap::new(),
    };
    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();

    let mut ensure_file = |nodes: &mut BTreeMap<String, Node>, f: u32| -> String {
        if let Some(id) = ids.file.get(&f) {
            return id.clone();
        }
        let info = g.file(f);
        let pid = ensure_package(nodes, g, info.package);
        ids.package.insert(info.package, pid.clone());
        let fid = format!("file:{}", info.path);
        let (link, cmd, web) = links(prov, &info.path, None, None);
        let mut n = Node::new(fid.clone(), NodeKind::File, info.path.clone());
        n.parent = Some(pid);
        n.path = Some(info.path.clone());
        n.package = Some(g.package_name(info.package).to_string());
        n.is_test = info.is_test;
        n.link = link;
        n.open_cmd = cmd;
        n.web_url = web;
        nodes.insert(fid.clone(), n);
        ids.file.insert(f, fid.clone());
        fid
    };

    for (&nref, &depth) in &w.visited {
        let id = id_of(g, nref);
        let focus = w.starts.contains(&nref);
        match nref {
            NodeRef::Decl(i) => {
                let d = g.decl(i);
                let at = Place {
                    parent: ensure_file(&mut nodes, d.file),
                    path: &g.file(d.file).path,
                    package: d.package,
                    is_test: d.is_test,
                    focus,
                    depth,
                };
                let n = located_node(
                    g,
                    prov,
                    nref,
                    d.qualified.clone(),
                    d.kind.as_str().to_string(),
                    Some((&d.qualified, d.line)),
                    at,
                );
                nodes.insert(id, n);
            }
            NodeRef::File(f) => {
                let info = g.file(f);
                let at = Place {
                    parent: ensure_file(&mut nodes, f),
                    path: &info.path,
                    package: info.package,
                    is_test: info.is_test,
                    focus,
                    depth,
                };
                let label = format!("{} (file level)", file_name(&info.path));
                let n = located_node(g, prov, nref, label, "file-level".into(), None, at);
                nodes.insert(id, n);
            }
            NodeRef::External(e) => {
                let name = g.externals[e as usize].clone();
                let mut n = Node::new(id.clone(), NodeKind::External, name.clone());
                n.depth = Some(depth);
                // A repository package's name: the references the index did not resolve sit
                // inside that package's node, labelled, so the package is drawn once.
                if let Some(p) = g.packages.iter().position(|p| same_package(&p.name, &name)) {
                    let pid = ensure_package(&mut nodes, g, p as u32);
                    n.unresolved_in_repo = true;
                    n.label = format!("{name} (unresolved)");
                    n.parent = Some(pid);
                    n.package = Some(g.package_name(p as u32).to_string());
                }
                nodes.insert(id, n);
            }
        }
    }

    // "+N more" nodes, one per package with dropped declarations.
    let mut more_id: HashMap<u32, String> = HashMap::new();
    for (&pkg, dropped) in &w.overflow {
        let pname = g.package_name(pkg).to_string();
        let pid = ensure_package(&mut nodes, g, pkg);
        let id = format!("more:{pname}");
        let mut n = Node::new(
            id.clone(),
            NodeKind::More,
            format!("+{} more in {pname}", dropped.len()),
        );
        n.parent = Some(pid);
        n.hidden = Some(dropped.len());
        n.package = Some(pname);
        nodes.insert(id.clone(), n);
        more_id.insert(pkg, id);
    }

    // Induced edges among kept nodes.
    let mut edges: Vec<Edge> = vec![];
    for &u in w.visited.keys() {
        for e in g.out_edges(u) {
            if spec.allows(e.kind) && w.visited.contains_key(&e.to) {
                edges.push(Edge {
                    from: id_of(g, u),
                    to: id_of(g, e.to),
                    kind: e.kind,
                    count: e.count,
                    status: edge_status(e.count, e.mapped),
                    mapped: e.mapped,
                    in_cycle: false,
                });
            }
        }
    }
    for (&(u, pkg, kind, outgoing), &(count, mapped)) in &w.more_edges {
        let m = more_id[&pkg].clone();
        let (from, to) = if outgoing {
            (id_of(g, u), m)
        } else {
            (m, id_of(g, u))
        };
        edges.push(Edge {
            from,
            to,
            kind,
            count,
            status: edge_status(count, mapped),
            mapped,
            in_cycle: false,
        });
    }
    edges.sort_by(|a, b| (&a.from, &a.to, a.kind).cmp(&(&b.from, &b.to, b.kind)));
    mark_cycles(&mut edges);

    let node_list: Vec<Node> = {
        let mut v: Vec<Node> = nodes.into_values().collect();
        let rank = |k: NodeKind| match k {
            NodeKind::Package => 0,
            NodeKind::File => 1,
            NodeKind::Symbol => 2,
            NodeKind::More => 3,
            NodeKind::External => 4,
        };
        v.sort_by(|a, b| (rank(a.kind), &a.id).cmp(&(rank(b.kind), &b.id)));
        v
    };
    let rollups = rollup(&node_list, &edges);

    if w.truncated {
        notes.push(format!(
            "Capped at {} declarations; the rest are the \"+N more\" nodes.",
            spec.max_nodes
        ));
    }
    if w.omitted_file_level > 0 {
        notes.push(format!(
            "{} import-like reference(s) from code outside any declaration are not drawn (they restate the edges shown); --file-level draws them.",
            w.omitted_file_level
        ));
    }
    match g.status {
        IndexStatus::Stale => notes.push("The index is stale: the checkout changed after it was built, so results may be out of date.".into()),
        IndexStatus::Partial => notes.push("The index is partial: it was built after a prepare step that failed, so edges in code that needed the prepared files may be missing.".into()),
        IndexStatus::Failed => notes.push("The last index build failed: this graph uses an earlier index, so results may be out of date or incomplete.".into()),
        IndexStatus::Current | IndexStatus::None => {}
    }
    if g.stats.unresolved_internal > 0 {
        notes.push(format!(
            "{} reference(s) to symbols the index has no definition for inside their own package (such as `Self` in impl blocks) are not drawn.",
            g.stats.unresolved_internal
        ));
    }
    if g.stats.mapped > 0 {
        notes.push(format!(
            "{} reference(s) use a workspace package's built declarations (dist) and are drawn to the source declaration they were mapped to; those edges say `mapped` (unverified: the index did not record the link).",
            g.stats.mapped
        ));
    }
    if g.stats.unreadable > 0 {
        notes.push(format!(
            "{} indexed file(s) could not be read now and contribute no edges.",
            g.stats.unreadable
        ));
    }
    let unresolved: usize = node_list.iter().map(|n| n.unresolved_calls.len()).sum();
    if unresolved > 0 {
        notes.push(format!(
            "{unresolved} call site(s) seen by Tree-sitter were not resolved by the index; they are listed on their nodes and drawn as no edge."
        ));
    }

    Graph {
        meta: Meta {
            schema: "cho-graph/1",
            cho_graph: env!("CARGO_PKG_VERSION"),
            provenance: prov.clone(),
            index_status: g.status,
            limits: LIMITS,
        },
        view: ViewInfo {
            kind,
            title,
            focus: w.starts.iter().map(|&n| id_of(g, n)).collect(),
            direction,
            depth: Some(spec.depth),
            edge_kinds: if spec.kinds.is_empty() {
                EdgeKind::ALL.to_vec()
            } else {
                spec.kinds.clone()
            },
            default_level: kind.default_level(),
            max_nodes: spec.max_nodes,
            truncated: w.truncated,
        },
        nodes: node_list,
        edges,
        rollups,
        reachable_tests: vec![],
        notes,
    }
}

fn capped(calls: &[UnresolvedCall]) -> Vec<UnresolvedCall> {
    let mut v: Vec<UnresolvedCall> = calls.to_vec();
    v.sort_by(|a, b| (a.line, &a.name).cmp(&(b.line, &b.name)));
    v.dedup();
    v.truncate(20);
    v
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Marks edges whose ends reach each other (strongly connected components).
fn mark_cycles(edges: &mut [Edge]) {
    let mut ix: HashMap<&str, usize> = HashMap::new();
    for e in edges.iter() {
        let n = ix.len();
        ix.entry(&e.from).or_insert(n);
        let n = ix.len();
        ix.entry(&e.to).or_insert(n);
    }
    let n = ix.len();
    let mut adj = vec![vec![]; n];
    for e in edges.iter() {
        adj[ix[e.from.as_str()]].push(ix[e.to.as_str()]);
    }
    let comp = scc(&adj);
    let flags: Vec<bool> = edges
        .iter()
        .map(|e| {
            let (a, b) = (ix[e.from.as_str()], ix[e.to.as_str()]);
            a != b && comp[a] == comp[b]
        })
        .collect();
    for (e, f) in edges.iter_mut().zip(flags) {
        e.in_cycle = f;
    }
}

/// Tarjan's algorithm, iterative. Returns a component number per node.
fn scc(adj: &[Vec<usize>]) -> Vec<usize> {
    let n = adj.len();
    let (mut index, mut low) = (vec![usize::MAX; n], vec![0; n]);
    let (mut on, mut comp) = (vec![false; n], vec![usize::MAX; n]);
    let (mut stack, mut next, mut ncomp) = (vec![], 0, 0);
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut work = vec![(root, 0usize)];
        while let Some(&mut (v, ref mut i)) = work.last_mut() {
            if *i == 0 {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on[v] = true;
            }
            if let Some(&u) = adj[v].get(*i) {
                *i += 1;
                if index[u] == usize::MAX {
                    work.push((u, 0));
                } else if on[u] {
                    low[v] = low[v].min(index[u]);
                }
            } else {
                work.pop();
                if let Some(&(p, _)) = work.last() {
                    low[p] = low[p].min(low[v]);
                }
                if low[v] == index[v] {
                    while let Some(u) = stack.pop() {
                        on[u] = false;
                        comp[u] = ncomp;
                        if u == v {
                            break;
                        }
                    }
                    ncomp += 1;
                }
            }
        }
    }
    comp
}

/// Edges aggregated to files and packages. Edges inside one file or package are dropped.
fn rollup(nodes: &[Node], edges: &[Edge]) -> Rollups {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let file_of = |id: &str| -> String {
        let n = by_id[id];
        match n.kind {
            NodeKind::Symbol => n.parent.clone().unwrap_or_else(|| n.id.clone()),
            _ => n.id.clone(),
        }
    };
    let package_of = |id: &str| -> String {
        let mut n = by_id[id];
        while let Some(p) = n.parent.as_deref().and_then(|p| by_id.get(p)) {
            n = p;
        }
        n.id.clone()
    };
    let agg = |rep: &dyn Fn(&str) -> String| -> Vec<Rollup> {
        let mut m: BTreeMap<(String, String), RollAcc> = BTreeMap::new();
        for e in edges {
            let (a, b) = (rep(&e.from), rep(&e.to));
            if a == b {
                continue;
            }
            let entry = m.entry((a, b)).or_default();
            entry.0 += e.count;
            entry.1.insert(&e.to);
            *entry.2.entry(e.kind.as_str().to_string()).or_default() += e.count;
        }
        m.into_iter()
            .map(|((from, to), (references, targets, kinds))| Rollup {
                from,
                to,
                references,
                symbols: targets.len() as u32,
                kinds,
            })
            .collect()
    };
    Rollups {
        file: agg(&file_of),
        package: agg(&package_of),
    }
}

/// `mapped` when every reference behind an edge came through a mapped built declaration.
fn edge_status(count: u32, mapped: u32) -> EdgeStatus {
    if count > 0 && mapped == count {
        EdgeStatus::Mapped
    } else {
        EdgeStatus::Verified
    }
}
