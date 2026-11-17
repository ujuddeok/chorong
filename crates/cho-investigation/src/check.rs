//! Resolving an investigation's anchors against a checkout.
//!
//! This is the syntactic half of verification: does each anchor name a real
//! file and declaration? Edges can only become [`Verification::Verified`] once
//! an [`EdgeOracle`] (a semantic index) confirms them, so without one every
//! edge between resolved nodes is reported as [`Verification::Claimed`].

use crate::{Evidence, Investigation, Relation, Verification};
use cho_core::SourceBuffer;
use cho_syntax::{Language, SymbolMatch, find_symbol};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeStatus {
    /// The anchor names exactly one place.
    Resolved,
    /// The symbol matches several declarations; the first is used.
    Ambiguous,
    /// The file or symbol doesn't exist at this checkout.
    Broken,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeReport {
    pub id: String,
    /// The anchor's repository-relative path.
    pub path: String,
    pub status: NodeStatus,
    /// Where the anchor resolved to (byte range of the declaration or excerpt).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<std::ops::Range<usize>>,
    /// The whole region the anchor covers: the declaration (for a symbol
    /// anchor), the line, or the file. Equals `range` unless an excerpt narrows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<std::ops::Range<usize>>,
    /// Byte range of the declared name, for symbol anchors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_range: Option<std::ops::Range<usize>>,
    /// One-based line of `range.start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    /// Version 2: the author's evidence, as written. Never changed by Chorong, and separate
    /// from `status`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_evidence: Option<Evidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EdgeReport {
    pub from: String,
    pub to: String,
    pub status: &'static str,
    /// The same label as `status`, typed.
    #[serde(skip)]
    pub verification: Verification,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub nodes: Vec<NodeReport>,
    pub edges: Vec<EdgeReport>,
    /// Format version 2 only: views, steps, external nodes, comparisons and branches.
    /// Absent for version 1 files, so their report is unchanged.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub v2: Option<crate::check_v2::V2Report>,
}

impl CheckReport {
    /// Broken node anchors.
    pub fn broken_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Broken)
            .count()
    }

    /// Broken node anchors plus, in version 2, broken anchors in views and the untraced
    /// list. Equals [`Self::broken_count`] for version 1.
    pub fn broken_total(&self) -> usize {
        self.broken_count() + self.v2.as_ref().map_or(0, |v| v.broken_anchors())
    }

    pub fn node(&self, id: &str) -> Option<&NodeReport> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// One end of an edge, as resolved against the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint<'a> {
    pub path: &'a str,
    /// Byte range of the declaration (or line, or file) the node anchors.
    pub region: std::ops::Range<usize>,
    /// Byte range of the declared name; `None` unless the anchor names a symbol.
    pub name_range: Option<std::ops::Range<usize>>,
}

/// Something that can confirm an edge from an index of the code, never by
/// guessing. `confirms` returns true only for positive evidence; "I don't know"
/// is `false` and leaves the edge `claimed`.
pub trait EdgeOracle {
    /// Is there a `rel` relationship from the code in `from` to the
    /// declaration at `to`? Only asked for `calls`, `references` and
    /// `dispatches` edges between nodes that resolved uniquely.
    fn confirms(&self, rel: Relation, from: &Endpoint<'_>, to: &Endpoint<'_>) -> bool;
}

fn verification_str(v: Verification) -> &'static str {
    match v {
        Verification::Verified => "verified",
        Verification::Claimed => "claimed",
        Verification::Broken => "broken",
    }
}

fn endpoint(n: &NodeReport) -> Option<Endpoint<'_>> {
    Some(Endpoint {
        path: n.path.as_str(),
        region: n.region.clone()?,
        name_range: n.name_range.clone(),
    })
}

/// Where checking reads the anchored files from: the working tree
/// ([`WorktreeReader`]) or a commit ([`crate::drift::CommitReader`]).
pub trait SourceReader {
    /// The bytes of the repository-relative `path`, or why it can't be read.
    fn read(&self, path: &str) -> Result<Vec<u8>, String>;

    /// The checkout the files come from, for parts that ask git about other revisions (diff
    /// panes). `None` when there is none; those parts are then reported as unresolved.
    fn root(&self) -> Option<&Path> {
        None
    }
}

/// Reads files from a checkout with [`cho_core::fs::read_source_in`]: regular files
/// only, size-capped, nothing outside `root`.
#[derive(Debug, Clone)]
pub struct WorktreeReader<'a> {
    pub root: &'a Path,
}

impl SourceReader for WorktreeReader<'_> {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        cho_core::fs::read_source_in(self.root, Path::new(path)).map_err(|e| e.to_string())
    }

    fn root(&self) -> Option<&Path> {
        Some(self.root)
    }
}

/// Checks every anchor against the files under `root`, which should be a
/// checkout of the investigation's `rev`.
pub fn check(inv: &Investigation, root: &Path) -> CheckReport {
    check_with_oracle(inv, root, None)
}

/// Like [`check`], but a `calls`, `references` or `dispatches` edge between
/// two uniquely resolved nodes becomes `verified` when `oracle` confirms it.
pub fn check_with_oracle(
    inv: &Investigation,
    root: &Path,
    oracle: Option<&dyn EdgeOracle>,
) -> CheckReport {
    check_with_reader(inv, &WorktreeReader { root }, oracle)
}

/// Like [`check_with_oracle`], reading files through `reader`. The oracle must describe
/// the same code the reader returns, or ranges won't line up; pass `None` when unsure.
pub fn check_with_reader(
    inv: &Investigation,
    reader: &dyn SourceReader,
    oracle: Option<&dyn EdgeOracle>,
) -> CheckReport {
    let nodes: Vec<NodeReport> = inv
        .nodes
        .iter()
        .map(|n| {
            let mut r = check_node(&n.id, &n.anchor, reader);
            r.author_evidence = n.evidence;
            r
        })
        .collect();
    let node_of = |id: &str| nodes.iter().find(|n| n.id == id);
    let status_of = |id: &str| node_of(id).map(|n| &n.status);
    let edges = inv
        .edges
        .iter()
        .map(|e| {
            let broken = [&e.from, &e.to]
                .iter()
                .any(|id| status_of(id) == Some(&NodeStatus::Broken));
            let confirmed = || -> Option<bool> {
                if !matches!(
                    e.rel,
                    Relation::Calls | Relation::References | Relation::Dispatches
                ) {
                    return None;
                }
                let (from, to) = (node_of(&e.from)?, node_of(&e.to)?);
                if from.status != NodeStatus::Resolved || to.status != NodeStatus::Resolved {
                    return None;
                }
                Some(oracle?.confirms(e.rel, &endpoint(from)?, &endpoint(to)?))
            };
            let v = if broken {
                Verification::Broken
            } else if confirmed() == Some(true) {
                Verification::Verified
            } else {
                Verification::Claimed
            };
            EdgeReport {
                from: e.from.clone(),
                to: e.to.clone(),
                status: verification_str(v),
                verification: v,
            }
        })
        .collect();
    let mut report = CheckReport {
        nodes,
        edges,
        v2: None,
    };
    if inv.cho >= 2 {
        report.v2 = Some(crate::check_v2::check(inv, &report, reader));
    }
    report
}

/// Resolves one anchor (a node's, or one written in a view). `id` names it in the report.
pub fn check_node(id: &str, anchor: &crate::Anchor, reader: &dyn SourceReader) -> NodeReport {
    let mut report = NodeReport {
        id: id.into(),
        path: anchor.path.clone(),
        status: NodeStatus::Broken,
        range: None,
        region: None,
        name_range: None,
        line: None,
        messages: vec![],
        author_evidence: None,
    };
    let buffer = match reader.read(&anchor.path) {
        Err(e) => {
            report
                .messages
                .push(format!("cannot read `{}`: {e}", anchor.path));
            return report;
        }
        Ok(bytes) => match SourceBuffer::from_bytes(bytes) {
            Ok(b) => b,
            Err(e) => {
                report.messages.push(format!("`{}`: {e}", anchor.path));
                return report;
            }
        },
    };
    let source = buffer.text();

    // Region the anchor covers: a declaration, or the whole file.
    let mut status = NodeStatus::Resolved;
    let mut region = 0..source.len();
    let mut name_range = None;
    if let Some(symbol) = &anchor.symbol {
        let Some(language) = Language::from_path(Path::new(&anchor.path)) else {
            report.messages.push(format!(
                "`{}` is not in a supported language, so `{symbol}` cannot be resolved",
                anchor.path
            ));
            return report;
        };
        match find_symbol(language, source, symbol) {
            SymbolMatch::Unique(s) => {
                name_range = Some(s.name_range.clone());
                region = s.range;
            }
            SymbolMatch::Ambiguous(all) => {
                status = NodeStatus::Ambiguous;
                report.messages.push(format!(
                    "`{symbol}` matches {} declarations; using the first",
                    all.len()
                ));
                name_range = Some(all[0].name_range.clone());
                region = all[0].range.clone();
            }
            SymbolMatch::NotFound => {
                report
                    .messages
                    .push(format!("no declaration `{symbol}` in `{}`", anchor.path));
                return report;
            }
        }
    } else if let Some(line) = anchor.line {
        match buffer.line_range(line - 1) {
            Some(r) => region = r,
            None => {
                report
                    .messages
                    .push(format!("`{}` has no line {line}", anchor.path));
                return report;
            }
        }
    }

    let mut target = region.clone();
    if let (Some(from), Some(to)) = (&anchor.from, &anchor.to) {
        let in_region = &source[region.clone()];
        match in_region.find(from.as_str()) {
            None => {
                status = NodeStatus::Broken;
                report.messages.push(
                    "range start (`from`) not found in the anchored code; the code may have changed"
                        .into(),
                );
            }
            Some(at) => match in_region[at + from.len()..].find(to.as_str()) {
                None => {
                    status = NodeStatus::Broken;
                    report.messages.push(
                        "range end (`to`) not found after `from` in the anchored code; the code may have changed"
                            .into(),
                    );
                }
                Some(end) => {
                    let end = region.start + at + from.len() + end + to.len();
                    target = region.start + at..end;
                }
            },
        }
    } else if let Some(excerpt) = &anchor.excerpt {
        match source[region.clone()].find(excerpt.as_str()) {
            Some(at) => target = region.start + at..region.start + at + excerpt.len(),
            None => {
                status = NodeStatus::Broken;
                report.messages.push(
                    "excerpt not found in the anchored code; the code may have changed".into(),
                );
            }
        }
    }
    report.status = status;
    report.region = Some(region);
    report.name_range = name_range;
    report.line = Some(buffer.line_of(target.start) + 1);
    report.range = Some(target);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_and_breaks() {
        let dir = std::env::temp_dir().join(format!("cho-check-{}", std::process::id()));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/queue.rs"),
            "struct Queue;\nimpl Queue {\n    fn push(&self) {\n        self.tx.send(job);\n    }\n}\nfn new() {}\nmod m { fn new() {} }\n",
        )
        .unwrap();
        let inv = Investigation::parse(
            &serde_json::json!({
                "chorong": 1, "title": "t", "kind": "diagram", "repo": "local:/x", "rev": "abcdef1",
                "nodes": [
                    { "id": "ok", "anchor": { "path": "src/queue.rs", "symbol": "Queue::push", "excerpt": "self.tx.send(job)" } },
                    { "id": "amb", "anchor": { "path": "src/queue.rs", "symbol": "new" } },
                    { "id": "nosym", "anchor": { "path": "src/queue.rs", "symbol": "Queue::pop" } },
                    { "id": "nofile", "anchor": { "path": "src/missing.rs" } },
                    { "id": "drift", "anchor": { "path": "src/queue.rs", "symbol": "Queue::push", "excerpt": "self.rx" } },
                    { "id": "line", "anchor": { "path": "src/queue.rs", "line": 7 } }
                ],
                "edges": [
                    { "from": "ok", "to": "amb", "rel": "calls" },
                    { "from": "ok", "to": "nofile", "rel": "calls" }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let report = check(&inv, &dir);
        let status = |id: &str| report.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(status("ok").status, NodeStatus::Resolved);
        assert_eq!(status("ok").line, Some(4));
        assert_eq!(status("amb").status, NodeStatus::Ambiguous);
        assert_eq!(status("nosym").status, NodeStatus::Broken);
        assert_eq!(status("nofile").status, NodeStatus::Broken);
        assert_eq!(status("drift").status, NodeStatus::Broken);
        assert_eq!(status("line").line, Some(7));
        assert_eq!(report.edges[0].status, "claimed");
        assert_eq!(report.edges[1].status, "broken");
        assert_eq!(report.broken_count(), 3);
        fs::remove_dir_all(dir).ok();
    }

    struct Always(bool);
    impl EdgeOracle for Always {
        fn confirms(&self, _: Relation, from: &Endpoint<'_>, to: &Endpoint<'_>) -> bool {
            // Endpoints carry real ranges: `from` is the whole method, `to` names a symbol.
            self.0 && from.region.len() > 10 && to.name_range.is_some()
        }
    }

    #[test]
    fn oracle_verifies_only_resolved_call_like_edges() {
        let dir = std::env::temp_dir().join(format!("cho-oracle-{}", std::process::id()));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src/q.rs"),
            "struct Q;\nimpl Q {\n    fn push(&self) {\n        helper();\n    }\n}\nfn helper() {}\nfn dup() {}\nmod m { fn dup() {} }\n",
        )
        .unwrap();
        let node = |id: &str, sym: &str| serde_json::json!({ "id": id, "anchor": { "path": "src/q.rs", "symbol": sym } });
        let edge = |from: &str, to: &str, rel: &str| serde_json::json!({ "from": from, "to": to, "rel": rel });
        let inv = Investigation::parse(
            &serde_json::json!({
                "chorong": 1, "title": "t", "kind": "diagram", "repo": "local:/x", "rev": "abcdef1",
                "nodes": [node("push", "Q::push"), node("helper", "helper"), node("dup", "dup"),
                          node("gone", "nope")],
                "edges": [edge("push", "helper", "calls"), edge("push", "helper", "data-flow"),
                          edge("push", "dup", "calls"), edge("push", "gone", "calls")]
            })
            .to_string(),
        )
        .unwrap();
        let status = |r: &CheckReport| r.edges.iter().map(|e| e.status).collect::<Vec<_>>();
        let yes = check_with_oracle(&inv, &dir, Some(&Always(true)));
        assert_eq!(status(&yes), ["verified", "claimed", "claimed", "broken"]);
        let no = check_with_oracle(&inv, &dir, Some(&Always(false)));
        assert_eq!(status(&no), ["claimed", "claimed", "claimed", "broken"]);
        assert_eq!(status(&check(&inv, &dir)), status(&no));
        fs::remove_dir_all(dir).ok();
    }
}
