//! Building the whole-repository reference graph from a loaded index.
//!
//! One pass over the index documents: each reference occurrence is attributed to the
//! innermost declaration (a Tree-sitter outline entry that has a SCIP definition) whose
//! range contains it, or to the file when it sits outside every declaration. Outbound and
//! inbound edges come from the same set. The kind of each edge comes from Tree-sitter, by
//! where the reference sits. Nothing is matched by name.

use crate::model::{EdgeKind, UnresolvedCall};
use crate::packages::{PackageDetector, PackageInfo, same_package};
use cho_core::SourceBuffer;
use cho_semantic::SymbolId;
use cho_semantic::symbol::package_of as symbol_package;
use cho_semantic::{DocOccurrence, IndexStatus, Semantic};
use cho_syntax::{Language, RefKind, RefToken, Symbol, SymbolKind, classify_references, outline};
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::Path;

/// A node of the whole graph, before any view or roll-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeRef {
    Decl(u32),
    /// Code in a file outside every declaration (imports, impl headers, statics).
    File(u32),
    /// A package that is not part of the repository.
    External(u32),
}

#[derive(Debug, Clone)]
pub struct Decl {
    pub symbol: SymbolId,
    pub name: String,
    pub qualified: String,
    pub kind: SymbolKind,
    pub file: u32,
    pub package: u32,
    /// One-based, of the name token.
    pub line: usize,
    /// One-based, inclusive: the whole declaration.
    pub start_line: usize,
    pub end_line: usize,
    pub is_test: bool,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: String,
    pub package: u32,
    pub is_test: bool,
}

#[derive(Debug, Clone)]
pub struct RawEdge {
    pub from: NodeRef,
    pub to: NodeRef,
    pub kind: EdgeKind,
    pub count: u32,
    /// Of `count`, references to a built declaration (`dist/x.d.ts`) that the index
    /// loader mapped to this source declaration. Unverified.
    pub mapped: u32,
}

#[derive(Debug, Default)]
pub struct BuildStats {
    pub documents: usize,
    /// Documents whose file could not be read now (deleted or moved since indexing).
    pub unreadable: usize,
    pub references: usize,
    pub unresolved_calls: usize,
    /// References to symbols of the source's own package that the index has no definition
    /// for (mostly `Self` in impl blocks). Not drawn; counted.
    pub unresolved_internal: usize,
    /// References resolved through a mapped built declaration (`dist` to `src`).
    pub mapped: usize,
}

/// Every declaration and reference of an index, in both directions.
#[derive(Debug)]
pub struct RefGraph {
    pub decls: Vec<Decl>,
    pub files: Vec<FileInfo>,
    pub packages: Vec<PackageInfo>,
    pub externals: Vec<String>,
    pub edges: Vec<RawEdge>,
    pub status: IndexStatus,
    pub stats: BuildStats,
    out_idx: HashMap<NodeRef, Vec<u32>>,
    in_idx: HashMap<NodeRef, Vec<u32>>,
    unresolved: HashMap<NodeRef, Vec<UnresolvedCall>>,
}

struct Pending {
    from: NodeRef,
    target: SymbolId,
    kind: EdgeKind,
}

struct FileScan {
    pending: Vec<Pending>,
    unresolved: Vec<(NodeRef, UnresolvedCall)>,
}

impl RefGraph {
    /// Builds the graph from `sem`, reading sources from `sem.root()`.
    pub fn build(sem: &Semantic) -> RefGraph {
        let mut paths: Vec<String> = sem
            .index()
            .documents()
            .iter()
            .map(|d| d.path.clone())
            .collect();
        paths.sort();
        paths.dedup();

        let mut detector = PackageDetector::new(sem.root());
        let mut packages: Vec<PackageInfo> = vec![];
        let mut package_ix: HashMap<PackageInfo, u32> = HashMap::new();
        let mut g = RefGraph {
            decls: vec![],
            files: vec![],
            packages: vec![],
            externals: vec![],
            edges: vec![],
            status: sem.status(),
            stats: BuildStats::default(),
            out_idx: HashMap::new(),
            in_idx: HashMap::new(),
            unresolved: HashMap::new(),
        };
        let mut owner: HashMap<SymbolId, NodeRef> = HashMap::new();
        let mut scans: Vec<FileScan> = vec![];

        for path in &paths {
            g.stats.documents += 1;
            let pkg = detector.package_of(path);
            let next = packages.len() as u32;
            let package = *package_ix.entry(pkg.clone()).or_insert_with(|| {
                packages.push(pkg);
                next
            });
            let file = g.files.len() as u32;
            g.files.push(FileInfo {
                path: path.clone(),
                package,
                is_test: is_test_path(path),
            });
            let source = cho_core::fs::read_source_in(sem.root(), Path::new(path))
                .ok()
                .and_then(|b| SourceBuffer::from_bytes(b).ok());
            let Some(source) = source else {
                g.stats.unreadable += 1;
                scans.push(FileScan {
                    pending: vec![],
                    unresolved: vec![],
                });
                continue;
            };
            let occs = sem.document_occurrences_in(path, &source);
            let language = Language::from_path(Path::new(path));
            let scan = g.scan_file(file, language, &source, &occs, &mut owner);
            scans.push(scan);
        }
        g.packages = packages;

        // Resolve targets now that every definition is known.
        let mut counts: BTreeMap<(NodeRef, NodeRef, EdgeKind), (u32, u32)> = BTreeMap::new();
        let mut external_ix: HashMap<String, u32> = HashMap::new();
        for scan in scans {
            for p in scan.pending {
                // A built declaration mapped to source draws to the source's node.
                let mapped_to = || {
                    sem.mapped_source(&p.target)
                        .and_then(|(s, _)| owner.get(s).copied())
                };
                let direct = owner.get(&p.target).copied();
                let via_map = direct.is_none().then(mapped_to).flatten();
                let mapped = via_map.is_some();
                let to = match direct.or(via_map) {
                    Some(n) => n,
                    None => {
                        let Some(name) = symbol_package(&p.target.symbol) else {
                            continue;
                        };
                        let own = g
                            .package_of(p.from)
                            .is_some_and(|pk| same_package(g.package_name(pk), name));
                        if own {
                            g.stats.unresolved_internal += 1;
                            continue;
                        }
                        external(&mut external_ix, name)
                    }
                };
                if to != p.from {
                    let c = counts.entry((p.from, to, p.kind)).or_default();
                    c.0 += 1;
                    if mapped {
                        c.1 += 1;
                        g.stats.mapped += 1;
                    }
                    g.stats.references += 1;
                }
            }
            for (from, call) in scan.unresolved {
                g.stats.unresolved_calls += 1;
                g.unresolved.entry(from).or_default().push(call);
            }
        }
        // `implements` from SCIP relationships.
        for (id, _) in sem.index().symbols() {
            let (Some(&from), Some(info)) = (owner.get(id), sem.index().info(id)) else {
                continue;
            };
            for rel in info.relationships.iter().filter(|r| r.is_implementation) {
                let target = SymbolId::new(&rel.symbol, "");
                let to = match owner.get(&target) {
                    Some(&n) => n,
                    None => {
                        let Some(name) = symbol_package(&rel.symbol) else {
                            continue;
                        };
                        external(&mut external_ix, name)
                    }
                };
                if to != from {
                    counts
                        .entry((from, to, EdgeKind::Implements))
                        .or_default()
                        .0 += 1;
                }
            }
        }
        let mut externals: Vec<(u32, String)> =
            external_ix.into_iter().map(|(n, i)| (i, n)).collect();
        externals.sort();
        g.externals = externals.into_iter().map(|(_, n)| n).collect();

        for ((from, to, kind), (count, mapped)) in counts {
            let ix = g.edges.len() as u32;
            g.edges.push(RawEdge {
                from,
                to,
                kind,
                count,
                mapped,
            });
            g.out_idx.entry(from).or_default().push(ix);
            g.in_idx.entry(to).or_default().push(ix);
        }
        g
    }

    fn scan_file(
        &mut self,
        file: u32,
        language: Option<Language>,
        source: &SourceBuffer,
        occs: &[DocOccurrence],
        owner: &mut HashMap<SymbolId, NodeRef>,
    ) -> FileScan {
        let text = source.text();
        let path_is_test = self.files[file as usize].is_test;
        let package = self.files[file as usize].package;

        // Declarations: outline entries that own a definition occurrence on their name.
        let symbols: Vec<Symbol> = language.map(|l| outline(l, text)).unwrap_or_default();
        let mut local: Vec<(Range<usize>, u32)> = vec![];
        // Definition occurrences by start, so each outline entry finds its own by binary
        // search: `(start, occurrence index)`.
        let mut defs: Vec<(usize, usize)> = occs
            .iter()
            .enumerate()
            .filter(|(_, o)| o.is_definition && !o.id.is_local())
            .map(|(i, o)| (o.range.start, i))
            .collect();
        defs.sort_unstable();
        for sym in symbols.iter().filter(|s| s.kind != SymbolKind::Impl) {
            // The first definition (in occurrence order) inside the name.
            let name = &sym.name_range;
            let from = defs.partition_point(|&(start, _)| start < name.start);
            let Some(def) = defs[from..]
                .iter()
                .take_while(|&&(start, _)| start <= name.end)
                .map(|&(_, i)| i)
                .filter(|&i| occs[i].range.end <= name.end)
                .min()
                .map(|i| &occs[i])
            else {
                continue;
            };
            if owner.contains_key(&def.id) {
                continue;
            }
            let language = language.expect("declarations imply a language");
            let ix = self.decls.len() as u32;
            self.decls.push(Decl {
                symbol: def.id.clone(),
                name: sym.name.clone(),
                qualified: sym.qualified_name(language),
                kind: sym.kind,
                file,
                package,
                line: source.byte_to_line_col(sym.name_range.start).line + 1,
                start_line: source.byte_to_line_col(sym.range.start).line + 1,
                end_line: source
                    .byte_to_line_col(sym.range.end.saturating_sub(1).max(sym.range.start))
                    .line
                    + 1,
                is_test: path_is_test || sym.scope.iter().any(|s| s == "tests" || s == "test"),
            });
            owner.insert(def.id.clone(), NodeRef::Decl(ix));
            local.push((sym.range.clone(), ix));
        }
        let innermost = |r: &Range<usize>| -> NodeRef {
            local
                .iter()
                .filter(|(d, _)| d.start <= r.start && r.end <= d.end)
                .min_by_key(|(d, _)| d.end - d.start)
                .map(|&(_, ix)| NodeRef::Decl(ix))
                .unwrap_or(NodeRef::File(file))
        };

        // Who owns each definition in this file: the innermost declaration around it.
        for o in occs.iter().filter(|o| o.is_definition && !o.id.is_local()) {
            owner
                .entry(o.id.clone())
                .or_insert_with(|| innermost(&o.range));
        }

        // References, with kinds from Tree-sitter.
        let tokens: Vec<RefToken> = language
            .and_then(|l| classify_references(l, text))
            .unwrap_or_default();
        let kind_at = |r: &Range<usize>| -> RefKind {
            let i = tokens.partition_point(|t| t.range.start <= r.start);
            match i.checked_sub(1).map(|i| &tokens[i]) {
                Some(t) if t.range.end > r.start => t.kind,
                _ => RefKind::References,
            }
        };
        let mut scan = FileScan {
            pending: vec![],
            unresolved: vec![],
        };
        for o in occs.iter().filter(|o| !o.is_definition && !o.id.is_local()) {
            if text.get(o.range.clone()).is_none_or(|t| t.contains('\n')) {
                continue;
            }
            scan.pending.push(Pending {
                from: innermost(&o.range),
                target: o.id.clone(),
                kind: kind_at(&o.range).into(),
            });
        }

        // Call sites Tree-sitter sees that no occurrence covers: unresolved, never guessed.
        let mut spans: Vec<&Range<usize>> = occs.iter().map(|o| &o.range).collect();
        spans.sort_by_key(|r| r.start);
        for t in tokens.iter().filter(|t| t.kind == RefKind::Calls) {
            let i = spans.partition_point(|r| r.start < t.range.end);
            let covered = spans[..i]
                .iter()
                .rev()
                .take(8)
                .any(|r| r.end > t.range.start);
            if !covered {
                scan.unresolved.push((
                    innermost(&t.range),
                    UnresolvedCall {
                        name: text[t.range.clone()].to_string(),
                        line: source.byte_to_line_col(t.range.start).line + 1,
                    },
                ));
            }
        }
        scan
    }

    pub fn out_edges(&self, n: NodeRef) -> impl Iterator<Item = &RawEdge> {
        self.out_idx
            .get(&n)
            .into_iter()
            .flatten()
            .map(|&i| &self.edges[i as usize])
    }

    pub fn in_edges(&self, n: NodeRef) -> impl Iterator<Item = &RawEdge> {
        self.in_idx
            .get(&n)
            .into_iter()
            .flatten()
            .map(|&i| &self.edges[i as usize])
    }

    pub fn unresolved_calls(&self, n: NodeRef) -> &[UnresolvedCall] {
        self.unresolved.get(&n).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn decl(&self, ix: u32) -> &Decl {
        &self.decls[ix as usize]
    }

    pub fn file(&self, ix: u32) -> &FileInfo {
        &self.files[ix as usize]
    }

    pub fn package_name(&self, ix: u32) -> &str {
        &self.packages[ix as usize].name
    }

    /// The file a node lives in (`None` for externals).
    pub fn file_of(&self, n: NodeRef) -> Option<u32> {
        match n {
            NodeRef::Decl(i) => Some(self.decl(i).file),
            NodeRef::File(f) => Some(f),
            NodeRef::External(_) => None,
        }
    }

    /// The repository package a node belongs to (`None` for externals).
    pub fn package_of(&self, n: NodeRef) -> Option<u32> {
        self.file_of(n).map(|f| self.file(f).package)
    }
}

/// The node of the external package `name`, numbered in order of first use.
fn external(external_ix: &mut HashMap<String, u32>, name: &str) -> NodeRef {
    let next = external_ix.len() as u32;
    NodeRef::External(*external_ix.entry(name.to_string()).or_insert(next))
}

/// Test files by path and name convention. Labelled as convention, never as coverage.
pub fn is_test_path(path: &str) -> bool {
    let mut comps = path.split('/');
    let file = comps.next_back().unwrap_or("");
    let stem = file.split('.').next().unwrap_or("");
    comps.any(|c| matches!(c, "tests" | "test" | "__tests__" | "spec"))
        || stem.ends_with("_test")
        || stem.starts_with("test_")
        || file.contains(".test.")
        || file.contains(".spec.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths() {
        for p in [
            "crates/a/tests/it.rs",
            "src/a.test.ts",
            "src/__tests__/a.ts",
            "pkg/test_a.py",
            "src/a_test.rs",
        ] {
            assert!(is_test_path(p), "{p}");
        }
        for p in ["src/lib.rs", "src/contest.rs", "src/latest/a.rs"] {
            assert!(!is_test_path(p), "{p}");
        }
    }
}
