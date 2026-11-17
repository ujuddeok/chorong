//! Queries over a loaded index, answered against the files as they are now.

use crate::cache::IndexStatus;
use crate::dist_link::{DistLinks, Mapping};
use crate::index::{OccRef, ScipIndex, SymbolId};
use cho_core::{LineCol, SourceBuffer};
use cho_investigation::Relation;
use cho_investigation::check::{EdgeOracle, Endpoint};
use serde::Serialize;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A place in a file. `range` is bytes; `line` and `column` are one-based,
/// the column counted in UTF-8 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub path: String,
    pub range: Range<usize>,
    pub line: usize,
    pub column: usize,
    pub is_definition: bool,
    /// Set when the occurrence belongs to another symbol that a built declaration is
    /// mapped to or from (`dist` to `src`): the index did not record this link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapped: Option<MappedFrom>,
}

/// The symbol an occurrence really belongs to, and how it was joined to the asked one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MappedFrom {
    /// The occurrence's own SCIP symbol.
    pub symbol: String,
    #[serde(flatten)]
    pub mapping: Mapping,
    /// Always `false`: a mapped link is not verified by the index.
    pub verified: bool,
}

/// The symbol under a position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolAt {
    #[serde(flatten)]
    pub id: SymbolId,
    pub range: Range<usize>,
    pub is_definition: bool,
}

/// One occurrence in a document, as returned by [`Semantic::document_occurrences`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocOccurrence {
    pub id: SymbolId,
    pub range: Range<usize>,
    pub is_definition: bool,
}

/// One result of [`Semantic::search`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolHit {
    #[serde(flatten)]
    pub id: SymbolId,
    pub display_name: String,
    pub kind: String,
    pub location: Location,
}

/// One project index that was loaded: where its project lives in the repository,
/// the commit it was built at, and how fresh it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexOrigin {
    /// Relative to the repository root, `/`-separated; empty for the root itself.
    pub project_root: String,
    pub commit: Option<String>,
    pub status: IndexStatus,
}

/// A `symbol_at` candidate: the answer, its span, and its (document, occurrence) tie-break.
type Candidate = (SymbolAt, (u32, u32), (usize, u32));

/// A loaded index plus the checkout it describes.
pub struct Semantic {
    index: ScipIndex,
    root: PathBuf,
    status: IndexStatus,
    origins: Vec<IndexOrigin>,
    links: DistLinks,
    buffers: Mutex<HashMap<String, Option<Arc<SourceBuffer>>>>,
    pub(crate) outlines: Mutex<crate::peek::OutlineCache>,
}

impl Semantic {
    /// `root` is the repository root that document paths are relative to.
    pub fn new(index: ScipIndex, root: impl Into<PathBuf>, status: IndexStatus) -> Self {
        Self {
            index,
            root: root.into(),
            status,
            origins: vec![],
            links: DistLinks::default(),
            buffers: Mutex::default(),
            outlines: Mutex::default(),
        }
    }

    /// Records which commit and status each project's index has, so answers can
    /// say which index they came from. Without origins, answers carry the overall
    /// status and no commit.
    pub fn with_origins(mut self, origins: Vec<IndexOrigin>) -> Self {
        self.origins = origins;
        self
    }

    /// Joins workspace packages' built declarations (`dist/*.d.ts`) to their source
    /// symbols (see [`crate::dist_link`]). Reads the workspace manifests, tsconfigs and
    /// declaration maps once; [`Semantic::open`] calls it.
    pub fn with_dist_links(mut self) -> Self {
        let roots: Vec<String> = self
            .origins
            .iter()
            .map(|o| o.project_root.clone())
            .collect();
        self.links = DistLinks::compute(&self.index, &self.root, &roots);
        self
    }

    /// The built-declaration links of this index.
    pub fn dist_links(&self) -> &DistLinks {
        &self.links
    }

    /// The source symbol a built-declaration symbol is mapped to, and how.
    pub fn mapped_source(&self, symbol: &SymbolId) -> Option<(&SymbolId, Mapping)> {
        self.links.source_of(symbol)
    }

    /// `symbol` and every symbol joined to it by a mapped link, with the mapping that joins
    /// each one to `symbol` (`None` for `symbol` itself), `symbol` first.
    fn family(&self, symbol: &SymbolId) -> Vec<(SymbolId, Option<Mapping>)> {
        let mut out = vec![(symbol.clone(), None)];
        let (source, own) = match self.links.source_of(symbol) {
            Some((s, m)) => {
                out.push((s.clone(), Some(m)));
                (s, Some(m))
            }
            None => (symbol, None),
        };
        for d in self.links.mapped_to(source) {
            if d != symbol {
                // Two built declarations of one source: the other's own link describes it.
                let m = self.links.source_of(d).map(|(_, m)| m).or(own);
                out.push((d.clone(), m));
            }
        }
        out
    }

    pub fn origins(&self) -> &[IndexOrigin] {
        &self.origins
    }

    pub fn index(&self) -> &ScipIndex {
        &self.index
    }

    /// How fresh the index is. Results from a `stale` index may be out of date.
    pub fn status(&self) -> IndexStatus {
        self.status
    }

    /// The current contents of `path`, or `None` if it can't be read safely.
    pub(crate) fn buffer(&self, path: &str) -> Option<Arc<SourceBuffer>> {
        if let Some(hit) = self.buffers.lock().ok()?.get(path) {
            return hit.clone();
        }
        let loaded = crate::relpath::is_safe_relative(path)
            .then(|| cho_core::fs::read_source_in(&self.root, Path::new(path)).ok())
            .flatten()
            .and_then(|b| SourceBuffer::from_bytes(b).ok())
            .map(Arc::new);
        self.buffers
            .lock()
            .ok()?
            .insert(path.to_string(), loaded.clone());
        loaded
    }

    /// Byte range of an occurrence against the current file; `None` when the
    /// file is gone or the range doesn't fit it (never clamped).
    fn byte_range(&self, at: OccRef) -> Option<(Range<usize>, Arc<SourceBuffer>)> {
        let buf = self.buffer(&self.index.document(at.doc).path)?;
        Some((Self::byte_range_in(&self.index, at, &buf)?, buf))
    }

    /// [`Semantic::byte_range`] against `buf`, the text of the occurrence's document.
    fn byte_range_in(index: &ScipIndex, at: OccRef, buf: &SourceBuffer) -> Option<Range<usize>> {
        let doc = index.document(at.doc);
        let occ = index.occurrence(at);
        let unit = doc.encoding.unit();
        let r = occ.range;
        let start = buf.line_col_to_byte_exact(
            LineCol {
                line: r.start_line as usize,
                col: r.start_col as usize,
            },
            unit,
        )?;
        let end = buf.line_col_to_byte_exact(
            LineCol {
                line: r.end_line as usize,
                col: r.end_col as usize,
            },
            unit,
        )?;
        (start <= end).then_some(start..end)
    }

    fn location(&self, at: OccRef) -> Option<Location> {
        let (range, buf) = self.byte_range(at)?;
        let lc = buf.byte_to_line_col(range.start);
        Some(Location {
            path: self.index.document(at.doc).path.clone(),
            line: lc.line + 1,
            column: lc.col + 1,
            range,
            is_definition: self.index.occurrence(at).is_definition(),
            mapped: None,
        })
    }

    fn mapped_location(&self, at: OccRef, via: &Option<(String, Mapping)>) -> Option<Location> {
        let mut l = self.location(at)?;
        l.mapped = via.as_ref().map(|(symbol, mapping)| MappedFrom {
            symbol: symbol.clone(),
            mapping: *mapping,
            verified: false,
        });
        Some(l)
    }

    /// The symbol whose single-line occurrence covers `byte` in `path`. The
    /// innermost occurrence wins when several overlap.
    pub fn symbol_at(&self, path: &str, byte: usize) -> Option<SymbolAt> {
        let buf = self.buffer(path)?;
        let mut best: Option<Candidate> = None;
        for &di in self.index.documents_for(path) {
            let doc = self.index.document(di as u32);
            let pos = buf.byte_to_line_col_in(byte, doc.encoding.unit());
            // Only occurrences on the caret's line that start at or before it can cover it.
            for (_, oi) in self.index.line_candidates(di as u32, pos.line, pos.col) {
                let occ = &doc.occurrences[oi as usize];
                if !occ.range.covers(pos.line, pos.col) {
                    continue;
                }
                let span = occ.range.span();
                // Equal spans: the earlier document, then the earlier occurrence, wins.
                if best
                    .as_ref()
                    .is_some_and(|(_, s, o)| (*s, *o) < (span, (di, oi)))
                {
                    continue;
                }
                let at = OccRef {
                    doc: di as u32,
                    occ: oi,
                };
                if let Some((range, _)) = self.byte_range(at)
                    && range.contains(&byte)
                {
                    best = Some((
                        SymbolAt {
                            id: SymbolId::new(&occ.symbol, &doc.path),
                            range,
                            is_definition: occ.is_definition(),
                        },
                        span,
                        (di, oi),
                    ));
                }
            }
        }
        best.map(|(s, _, _)| s)
    }

    /// Where `symbol` is defined, in path and position order. Empty for
    /// symbols the index only references (the standard library, dependencies).
    /// A built declaration with no definition of its own (`dist/x.d.ts`) that is
    /// mapped to source gives the source definitions, marked [`Location::mapped`].
    pub fn definitions(&self, symbol: &SymbolId) -> Vec<Location> {
        let own = self
            .index
            .occurrences_of(symbol)
            .map(|occs| sorted(occs.definitions.iter().filter_map(|&a| self.location(a))))
            .unwrap_or_default();
        if !own.is_empty() {
            return own;
        }
        let Some((source, mapping)) = self.links.source_of(symbol) else {
            return own;
        };
        let via = Some((source.symbol.clone(), mapping));
        self.index
            .occurrences_of(source)
            .map(|occs| {
                sorted(
                    occs.definitions
                        .iter()
                        .filter_map(|&a| self.mapped_location(a, &via)),
                )
            })
            .unwrap_or_default()
    }

    /// Every occurrence of `symbol`: definitions first, then references. With mapped
    /// built declarations, the references of every symbol joined to `symbol` too, each
    /// marked with the symbol it belongs to ([`Location::mapped`]).
    pub fn references(&self, symbol: &SymbolId) -> Vec<Location> {
        let family = self.family(symbol);
        if family.len() == 1 && self.index.occurrences_of(symbol).is_none() {
            return vec![];
        }
        let mut all = self.definitions(symbol);
        let mut refs = vec![];
        for (id, mapping) in &family {
            let Some(occs) = self.index.occurrences_of(id) else {
                continue;
            };
            let via = mapping.map(|m| (id.symbol.clone(), m));
            refs.extend(
                occs.references
                    .iter()
                    .filter_map(|&a| self.mapped_location(a, &via)),
            );
        }
        all.extend(sorted(refs.into_iter()));
        all
    }

    /// Display name, kind and docs of a symbol, if the index has them.
    pub fn info(&self, symbol: &SymbolId) -> Option<&crate::index::SymbolInfo> {
        self.index.info(symbol)
    }

    /// The repository root that document paths are relative to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every named occurrence in the index documents for `path`, with its byte range
    /// against the file as it is now, in document then occurrence order. Occurrences
    /// whose range doesn't fit the current file are left out, never clamped.
    pub fn document_occurrences(&self, path: &str) -> Vec<DocOccurrence> {
        match self.buffer(path) {
            Some(buf) => self.document_occurrences_in(path, &buf),
            None => vec![],
        }
    }

    /// [`Semantic::document_occurrences`] against `text`, the caller's copy of the file
    /// as it is now. Reads nothing and caches nothing, so a caller that already read the
    /// file (the graph build reads every indexed file) doesn't read it twice.
    pub fn document_occurrences_in(&self, path: &str, text: &SourceBuffer) -> Vec<DocOccurrence> {
        let mut out = vec![];
        for &di in self.index.documents_for(path) {
            let doc = self.index.document(di as u32);
            for (oi, occ) in doc.occurrences.iter().enumerate() {
                if occ.symbol.is_empty() {
                    continue;
                }
                let at = OccRef {
                    doc: di as u32,
                    occ: oi as u32,
                };
                if let Some(range) = Self::byte_range_in(&self.index, at, text) {
                    out.push(DocOccurrence {
                        id: SymbolId::new(&occ.symbol, &doc.path),
                        range,
                        is_definition: occ.is_definition(),
                    });
                }
            }
        }
        out
    }

    /// Case-insensitive substring match on display names, definitions only.
    /// Exact matches come first, then prefix matches, then the rest.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SymbolHit> {
        let needle = query.to_lowercase();
        if needle.is_empty() {
            return vec![];
        }
        let mut found: Vec<(u8, &SymbolId, String, &crate::index::SymbolInfo)> = self
            .index
            .symbols()
            .filter(|(_, occs)| !occs.definitions.is_empty())
            .filter_map(|(id, _)| {
                let info = self.index.info(id)?;
                // scip-typescript leaves `display_name` empty; use the symbol's own name.
                let shown = if info.display_name.is_empty() {
                    crate::hover::symbol_name(&id.symbol).unwrap_or_default()
                } else {
                    info.display_name.clone()
                };
                let name = shown.to_lowercase();
                let rank = if name == needle {
                    0
                } else if name.starts_with(&needle) {
                    1
                } else if name.contains(&needle) {
                    2
                } else {
                    return None;
                };
                Some((rank, id, shown, info))
            })
            .collect();
        found.sort_by(|a, b| (a.0, &a.2, a.1).cmp(&(b.0, &b.2, b.1)));
        let mut hits = vec![];
        for (_, id, name, info) in found {
            if hits.len() >= limit {
                break;
            }
            if let Some(location) = self.definitions(id).into_iter().next() {
                hits.push(SymbolHit {
                    id: id.clone(),
                    display_name: name,
                    kind: info.kind.clone(),
                    location,
                });
            }
        }
        hits
    }

    /// Byte ranges of every occurrence (definitions and references) of `symbol` in
    /// `path`, sorted. Looks only at the symbol's own occurrences, so it is cheap even
    /// for a symbol used across thousands of files.
    /// Includes the occurrences of symbols joined to `symbol` by a mapped link.
    pub fn occurrences_in(&self, symbol: &SymbolId, path: &str) -> Vec<Range<usize>> {
        let mut found: Vec<Range<usize>> = vec![];
        for (id, _) in self.family(symbol) {
            let Some(occs) = self.index.occurrences_of(&id) else {
                continue;
            };
            found.extend(
                occs.definitions
                    .iter()
                    .chain(&occs.references)
                    .filter(|a| self.index.document(a.doc).path == path)
                    .filter_map(|&a| self.byte_range(a).map(|(r, _)| r)),
            );
        }
        found.sort_by_key(|r| (r.start, r.end));
        found.dedup();
        found
    }

    /// Is `symbol` referenced (not defined) inside `range` of `path`? Only the symbol's
    /// own occurrences count: a mapped link is not verification.
    pub fn references_within(&self, symbol: &SymbolId, path: &str, range: Range<usize>) -> bool {
        let Some(occs) = self.index.occurrences_of(symbol) else {
            return false;
        };
        occs.references.iter().any(|&a| {
            self.index.document(a.doc).path == path
                && self
                    .byte_range(a)
                    .is_some_and(|(r, _)| range.start <= r.start && r.end <= range.end)
        })
    }

    /// Symbols with a definition occurrence inside `name_range` in `path`.
    pub fn definitions_at(&self, path: &str, name_range: Range<usize>) -> Vec<SymbolId> {
        let Some(buf) = self.buffer(path) else {
            return vec![];
        };
        let mut found = vec![];
        for &di in self.index.documents_for(path) {
            let doc = self.index.document(di as u32);
            let line = buf
                .byte_to_line_col_in(name_range.start, doc.encoding.unit())
                .line;
            for (oi, occ) in doc.occurrences.iter().enumerate() {
                if !occ.is_definition()
                    || occ.symbol.is_empty()
                    || occ.range.start_line as usize != line
                {
                    continue;
                }
                let at = OccRef {
                    doc: di as u32,
                    occ: oi as u32,
                };
                if let Some((r, _)) = self.byte_range(at) {
                    let inside = name_range.start <= r.start && r.end <= name_range.end;
                    let id = SymbolId::new(&occ.symbol, &doc.path);
                    if inside && !found.contains(&id) {
                        found.push(id);
                    }
                }
            }
        }
        found
    }
}

fn sorted(it: impl Iterator<Item = Location>) -> Vec<Location> {
    let mut v: Vec<_> = it.collect();
    v.sort_by(|a, b| (&a.path, a.range.start).cmp(&(&b.path, b.range.start)));
    v.dedup();
    v
}

impl EdgeOracle for Semantic {
    /// Confirms the edge when the index has a reference to the symbol defined
    /// at `to`'s name inside `from`'s declaration. Anything else is `false`.
    fn confirms(&self, rel: Relation, from: &Endpoint<'_>, to: &Endpoint<'_>) -> bool {
        if !matches!(
            rel,
            Relation::Calls | Relation::References | Relation::Dispatches
        ) {
            return false;
        }
        let Some(name) = &to.name_range else {
            return false;
        };
        self.definitions_at(to.path, name.clone())
            .iter()
            .any(|s| self.references_within(s, from.path, from.region.clone()))
    }
}
