//! The in-memory form of one or more SCIP indexes.
//!
//! Positions stay as the raw SCIP ranges here; they become byte offsets only
//! in [`crate::Semantic`], through `SourceBuffer`, against the file as it is
//! read now.

use cho_core::ColumnUnit;
use protobuf::Message;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use crate::{SemanticError, relpath};

pub const ROLE_DEFINITION: i32 = 1;

/// A SCIP symbol, made unique across documents: `local N` symbols exist only
/// inside the document that mentions them, so they carry that document's path
/// and never match anywhere else.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct SymbolId {
    pub symbol: String,
    /// The document path, for local symbols only.
    pub local_in: Option<String>,
}

impl SymbolId {
    pub fn new(symbol: &str, doc_path: &str) -> Self {
        Self {
            symbol: symbol.to_string(),
            local_in: is_local(symbol).then(|| doc_path.to_string()),
        }
    }

    pub fn is_local(&self) -> bool {
        self.local_in.is_some()
    }
}

pub fn is_local(symbol: &str) -> bool {
    symbol.starts_with("local ")
}

/// A SCIP range, `[line, startCol, endLine?, endCol]`, zero-based, in the
/// document's position encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawRange {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

impl RawRange {
    /// Three elements mean a single-line range. Anything else, or any negative
    /// or inverted value, is malformed and yields `None`.
    pub fn from_scip(r: &[i32]) -> Option<Self> {
        let (sl, sc, el, ec) = match *r {
            [sl, sc, ec] => (sl, sc, sl, ec),
            [sl, sc, el, ec] => (sl, sc, el, ec),
            _ => return None,
        };
        if [sl, sc, el, ec].iter().any(|v| *v < 0) || (el, ec) < (sl, sc) {
            return None;
        }
        Some(Self {
            start_line: sl as u32,
            start_col: sc as u32,
            end_line: el as u32,
            end_col: ec as u32,
        })
    }

    /// Does the range cover the position `(line, col)` (start inclusive, end exclusive)?
    pub fn covers(&self, line: usize, col: usize) -> bool {
        let p = (line as u64, col as u64);
        (self.start_line as u64, self.start_col as u64) <= p
            && p < (self.end_line as u64, self.end_col as u64)
    }

    /// A size to rank overlapping occurrences by: lines first, then columns.
    pub fn span(&self) -> (u32, u32) {
        let lines = self.end_line - self.start_line;
        let cols = if lines == 0 {
            self.end_col - self.start_col
        } else {
            self.end_col
        };
        (lines, cols)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
    Utf32,
}

impl Encoding {
    /// `Unspecified` means UTF-16 in practice.
    pub fn unit(self) -> ColumnUnit {
        match self {
            Encoding::Utf8 => ColumnUnit::Utf8,
            Encoding::Utf16 => ColumnUnit::Utf16,
            Encoding::Utf32 => ColumnUnit::Utf32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub range: RawRange,
    pub symbol: String,
    pub roles: i32,
}

impl Occurrence {
    pub fn is_definition(&self) -> bool {
        self.roles & ROLE_DEFINITION != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Repository-relative, `/`-separated.
    pub path: String,
    pub encoding: Encoding,
    pub occurrences: Vec<Occurrence>,
}

fn line_table(doc: &Document) -> Vec<(u32, u32, u32)> {
    let mut t: Vec<(u32, u32, u32)> = doc
        .occurrences
        .iter()
        .enumerate()
        .filter(|(_, o)| !o.symbol.is_empty() && o.range.start_line == o.range.end_line)
        .map(|(i, o)| (o.range.start_line, o.range.start_col, i as u32))
        .collect();
    t.sort_unstable();
    t
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relationship {
    pub symbol: String,
    pub is_reference: bool,
    pub is_implementation: bool,
    pub is_type_definition: bool,
    pub is_definition: bool,
}

/// A symbol's signature as the indexer recorded it (SCIP `signature_documentation`):
/// a code snippet such as `fn helper() -> u32`, and the language it is written in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Signature {
    pub text: String,
    pub language: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymbolInfo {
    pub display_name: String,
    pub kind: String,
    pub documentation: Vec<String>,
    /// `None` when the indexer recorded no signature (empty text counts as none).
    pub signature: Option<Signature>,
    pub relationships: Vec<Relationship>,
}

impl SymbolInfo {
    /// Fills fields this entry lacks from another entry for the same symbol
    /// (another project's index). Fields already present are kept.
    fn fill_from(&mut self, other: SymbolInfo) {
        if self.display_name.is_empty() {
            self.display_name = other.display_name;
        }
        if self.kind.is_empty() || self.kind == "UnspecifiedKind" {
            self.kind = other.kind;
        }
        if self.documentation.is_empty() {
            self.documentation = other.documentation;
        }
        if self.signature.is_none() {
            self.signature = other.signature;
        }
        if self.relationships.is_empty() {
            self.relationships = other.relationships;
        }
    }
}

/// Where an occurrence lives: document index, occurrence index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OccRef {
    pub doc: u32,
    pub occ: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolOccurrences {
    pub definitions: Vec<OccRef>,
    pub references: Vec<OccRef>,
}

/// Documents, symbol information, and a map from symbol to its occurrences.
#[derive(Debug, Default)]
pub struct ScipIndex {
    docs: Vec<Document>,
    by_path: HashMap<String, Vec<usize>>,
    infos: HashMap<SymbolId, SymbolInfo>,
    occurrences: HashMap<SymbolId, SymbolOccurrences>,
    /// Per document: its single-line, named occurrences as `(line, start_col, occ)`,
    /// sorted, so a position lookup is a binary search by line.
    by_line: Vec<Vec<(u32, u32, u32)>>,
}

impl ScipIndex {
    /// Builds the lookup maps. `infos` are the symbol-information entries,
    /// keyed like [`SymbolId`] (use [`SymbolId::new`] with the document path
    /// that declared them; external symbols have no document and are never local).
    pub fn new(docs: Vec<Document>, infos: HashMap<SymbolId, SymbolInfo>) -> Self {
        let mut index = Self {
            docs,
            infos,
            ..Self::default()
        };
        index.index_docs_from(0);
        index
    }

    /// Adds the lookup entries of the documents from `first` on; the earlier ones are
    /// already in the tables.
    fn index_docs_from(&mut self, first: usize) {
        self.by_line
            .extend(self.docs[first..].iter().map(line_table));
        for (di, doc) in self.docs.iter().enumerate().skip(first) {
            self.by_path.entry(doc.path.clone()).or_default().push(di);
            for (oi, occ) in doc.occurrences.iter().enumerate() {
                if occ.symbol.is_empty() {
                    continue;
                }
                let at = OccRef {
                    doc: di as u32,
                    occ: oi as u32,
                };
                let entry = self
                    .occurrences
                    .entry(SymbolId::new(&occ.symbol, &doc.path))
                    .or_default();
                if occ.is_definition() {
                    entry.definitions.push(at);
                } else {
                    entry.references.push(at);
                }
            }
        }
    }

    /// Adds another index (another project of the same repository). Only the new
    /// documents are added to the lookup tables, so merging N projects stays linear.
    pub fn merge(&mut self, other: ScipIndex) {
        let first = self.docs.len();
        self.docs.extend(other.docs);
        for (k, v) in other.infos {
            match self.infos.entry(k) {
                std::collections::hash_map::Entry::Occupied(mut e) => e.get_mut().fill_from(v),
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(v);
                }
            }
        }
        self.index_docs_from(first);
    }

    /// Decodes a `.scip` file. Document paths are prefixed with `prefix`
    /// (the project root relative to the repository root; empty for the root).
    pub fn from_scip_bytes(bytes: &[u8], prefix: &str) -> Result<Self, SemanticError> {
        let raw = scip::types::Index::parse_from_bytes(bytes)
            .map_err(|e| SemanticError::Decode(e.to_string()))?;
        let mut docs = Vec::with_capacity(raw.documents.len());
        let mut infos = HashMap::new();
        for d in raw.documents {
            let path = relpath::join(relpath::trim_root(prefix), &d.relative_path);
            let encoding = match d.position_encoding.enum_value_or_default() {
                scip::types::PositionEncoding::UTF8CodeUnitOffsetFromLineStart => Encoding::Utf8,
                scip::types::PositionEncoding::UTF32CodeUnitOffsetFromLineStart => Encoding::Utf32,
                _ => Encoding::Utf16,
            };
            let occurrences = d
                .occurrences
                .into_iter()
                .filter_map(|o| {
                    Some(Occurrence {
                        range: RawRange::from_scip(&o.range)?,
                        symbol: o.symbol,
                        roles: o.symbol_roles,
                    })
                })
                .collect();
            for s in d.symbols {
                infos.insert(SymbolId::new(&s.symbol, &path), convert_info(s));
            }
            docs.push(Document {
                path,
                encoding,
                occurrences,
            });
        }
        for s in raw.external_symbols {
            infos.insert(SymbolId::new(&s.symbol, ""), convert_info(s));
        }
        Ok(Self::new(docs, infos))
    }

    #[allow(clippy::disallowed_methods)] // an index file Chorong wrote or the reader named; indexes can exceed the source size limit
    pub fn load(path: &Path, prefix: &str) -> Result<Self, SemanticError> {
        let bytes = std::fs::read(path).map_err(|e| SemanticError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        Self::from_scip_bytes(&bytes, prefix)
    }

    pub fn documents(&self) -> &[Document] {
        &self.docs
    }

    pub fn document(&self, at: u32) -> &Document {
        &self.docs[at as usize]
    }

    /// Indexes of the documents for `path` (usually one).
    pub fn documents_for(&self, path: &str) -> &[usize] {
        self.by_path.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Occurrences of document `doc` on `line` that start at or before `col`, as
    /// `(start_col, occurrence index)` in start order. Only single-line occurrences
    /// with a symbol are listed. Binary search by line, then a scan of that line.
    pub fn line_candidates(
        &self,
        doc: u32,
        line: usize,
        col: usize,
    ) -> impl Iterator<Item = (u32, u32)> + '_ {
        let table = &self.by_line[doc as usize];
        let line = u32::try_from(line).unwrap_or(u32::MAX);
        let col = u32::try_from(col).unwrap_or(u32::MAX);
        let from = table.partition_point(|&(l, _, _)| l < line);
        table[from..]
            .iter()
            .take_while(move |&&(l, c, _)| l == line && c <= col)
            .map(|&(_, c, o)| (c, o))
    }

    pub fn occurrence(&self, at: OccRef) -> &Occurrence {
        &self.docs[at.doc as usize].occurrences[at.occ as usize]
    }

    pub fn info(&self, id: &SymbolId) -> Option<&SymbolInfo> {
        self.infos.get(id)
    }

    pub fn occurrences_of(&self, id: &SymbolId) -> Option<&SymbolOccurrences> {
        self.occurrences.get(id)
    }

    pub fn symbols(&self) -> impl Iterator<Item = (&SymbolId, &SymbolOccurrences)> {
        self.occurrences.iter()
    }

    pub fn symbol_count(&self) -> usize {
        self.occurrences.len()
    }
}

fn convert_info(s: scip::types::SymbolInformation) -> SymbolInfo {
    SymbolInfo {
        display_name: s.display_name,
        kind: format!("{:?}", s.kind.enum_value_or_default()),
        documentation: s.documentation,
        signature: s
            .signature_documentation
            .into_option()
            .filter(|sig| !sig.text.is_empty())
            .map(|sig| Signature {
                text: sig.text,
                language: sig.language,
            }),
        relationships: s
            .relationships
            .into_iter()
            .map(|r| Relationship {
                symbol: r.symbol,
                is_reference: r.is_reference,
                is_implementation: r.is_implementation,
                is_type_definition: r.is_type_definition,
                is_definition: r.is_definition,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scip::types as pb;

    #[test]
    fn ranges() {
        assert_eq!(
            RawRange::from_scip(&[1, 2, 5]),
            Some(RawRange {
                start_line: 1,
                start_col: 2,
                end_line: 1,
                end_col: 5
            })
        );
        assert!(RawRange::from_scip(&[1, 2, 3, 4]).unwrap().covers(2, 100));
        assert!(RawRange::from_scip(&[1, 2]).is_none());
        assert!(RawRange::from_scip(&[1, 5, 1, 2]).is_none());
        assert!(RawRange::from_scip(&[-1, 0, 1]).is_none());
        let r = RawRange::from_scip(&[0, 2, 6]).unwrap();
        assert!(r.covers(0, 2) && r.covers(0, 5) && !r.covers(0, 6) && !r.covers(0, 1));
    }

    #[test]
    fn locals_are_scoped_to_their_document() {
        assert_ne!(
            SymbolId::new("local 1", "a.rs"),
            SymbolId::new("local 1", "b.rs")
        );
        assert_eq!(
            SymbolId::new("pkg/x.", "a.rs"),
            SymbolId::new("pkg/x.", "b.rs")
        );
    }

    /// The position encoding each document declares; scip-typescript leaves it unspecified
    /// and means UTF-16 (the SCIP default), so unspecified must not be read as UTF-8.
    #[test]
    fn decodes_each_position_encoding() {
        let mut idx = pb::Index::new();
        for (path, enc) in [
            (
                "u8.rs",
                pb::PositionEncoding::UTF8CodeUnitOffsetFromLineStart,
            ),
            (
                "u16.ts",
                pb::PositionEncoding::UTF16CodeUnitOffsetFromLineStart,
            ),
            (
                "u32.py",
                pb::PositionEncoding::UTF32CodeUnitOffsetFromLineStart,
            ),
            ("none.ts", pb::PositionEncoding::UnspecifiedPositionEncoding),
        ] {
            let mut doc = pb::Document::new();
            doc.relative_path = path.into();
            doc.position_encoding = enc.into();
            idx.documents.push(doc);
        }
        let loaded = ScipIndex::from_scip_bytes(&idx.write_to_bytes().unwrap(), "").unwrap();
        let got: Vec<_> = loaded
            .documents()
            .iter()
            .map(|d| (d.path.as_str(), d.encoding))
            .collect();
        assert_eq!(
            got,
            [
                ("u8.rs", Encoding::Utf8),
                ("u16.ts", Encoding::Utf16),
                ("u32.py", Encoding::Utf32),
                ("none.ts", Encoding::Utf16),
            ]
        );
    }

    #[test]
    fn decodes_scip_bytes() {
        let mut idx = pb::Index::new();
        let mut doc = pb::Document::new();
        doc.relative_path = "src/a.rs".into();
        doc.position_encoding = pb::PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into();
        let mut def = pb::Occurrence::new();
        def.range = vec![0, 3, 6];
        def.symbol = "rust-analyzer cargo p 0.1 a/foo().".into();
        def.symbol_roles = 1;
        let mut bad = pb::Occurrence::new();
        bad.range = vec![0];
        bad.symbol = "x".into();
        doc.occurrences = vec![def, bad];
        let mut info = pb::SymbolInformation::new();
        info.symbol = "rust-analyzer cargo p 0.1 a/foo().".into();
        info.display_name = "foo".into();
        info.kind = pb::symbol_information::Kind::Function.into();
        let mut sig = pb::Signature::new();
        sig.text = "fn foo()".into();
        sig.language = "rust".into();
        info.signature_documentation = Some(sig).into();
        let mut bare = pb::SymbolInformation::new();
        bare.symbol = "rust-analyzer cargo p 0.1 a/bar().".into();
        bare.signature_documentation = Some(pb::Signature::new()).into();
        doc.symbols = vec![info, bare];
        idx.documents = vec![doc];
        let bytes = idx.write_to_bytes().unwrap();

        let loaded = ScipIndex::from_scip_bytes(&bytes, "proj").unwrap();
        assert_eq!(loaded.documents()[0].path, "proj/src/a.rs");
        assert_eq!(loaded.documents()[0].encoding, Encoding::Utf8);
        assert_eq!(
            loaded.documents()[0].occurrences.len(),
            1,
            "malformed range dropped"
        );
        let id = SymbolId::new("rust-analyzer cargo p 0.1 a/foo().", "");
        assert_eq!(loaded.info(&id).unwrap().display_name, "foo");
        assert_eq!(loaded.info(&id).unwrap().kind, "Function");
        assert_eq!(
            loaded.info(&id).unwrap().signature,
            Some(Signature {
                text: "fn foo()".into(),
                language: "rust".into()
            })
        );
        let bare = SymbolId::new("rust-analyzer cargo p 0.1 a/bar().", "");
        assert_eq!(
            loaded.info(&bare).unwrap().signature,
            None,
            "an empty signature is no signature"
        );
        assert_eq!(loaded.occurrences_of(&id).unwrap().definitions.len(), 1);
        assert!(ScipIndex::from_scip_bytes(b"\xff\xff\xff", "").is_err());
    }

    #[test]
    fn merging_matches_building_once() {
        let doc = |path: &str, sym: &str| Document {
            path: path.into(),
            encoding: Encoding::Utf8,
            occurrences: vec![
                Occurrence {
                    range: RawRange::from_scip(&[0, 0, 3]).unwrap(),
                    symbol: sym.into(),
                    roles: ROLE_DEFINITION,
                },
                Occurrence {
                    range: RawRange::from_scip(&[2, 4, 7]).unwrap(),
                    symbol: "shared.".into(),
                    roles: 0,
                },
            ],
        };
        let all = vec![doc("a.rs", "a."), doc("b.rs", "b."), doc("a.rs", "c.")];
        let once = ScipIndex::new(all.clone(), HashMap::new());
        let mut merged = ScipIndex::new(vec![all[0].clone()], HashMap::new());
        merged.merge(ScipIndex::new(vec![all[1].clone()], HashMap::new()));
        merged.merge(ScipIndex::new(vec![all[2].clone()], HashMap::new()));
        assert_eq!(merged.documents(), once.documents());
        assert_eq!(merged.documents_for("a.rs"), once.documents_for("a.rs"));
        assert_eq!(merged.symbol_count(), once.symbol_count());
        for (id, occs) in once.symbols() {
            let m = merged.occurrences_of(id).unwrap();
            assert_eq!(m.definitions, occs.definitions, "{id:?}");
            assert_eq!(m.references, occs.references, "{id:?}");
        }
        for doc in 0..3 {
            let a: Vec<_> = merged.line_candidates(doc, 2, 9).collect();
            assert_eq!(a, once.line_candidates(doc, 2, 9).collect::<Vec<_>>());
            assert_eq!(a.len(), 1);
        }
    }

    #[test]
    fn merge_fills_missing_fields_and_keeps_present_ones() {
        let id = SymbolId::new("pkg x.", "");
        let mut a = HashMap::new();
        a.insert(
            id.clone(),
            SymbolInfo {
                display_name: "x".into(),
                ..SymbolInfo::default()
            },
        );
        let mut b = HashMap::new();
        let sig = Signature {
            text: "const x: u8".into(),
            language: "rust".into(),
        };
        b.insert(
            id.clone(),
            SymbolInfo {
                display_name: "other".into(),
                signature: Some(sig.clone()),
                ..SymbolInfo::default()
            },
        );
        let mut merged = ScipIndex::new(vec![], a);
        merged.merge(ScipIndex::new(vec![], b));
        let info = merged.info(&id).unwrap();
        assert_eq!(info.display_name, "x");
        assert_eq!(info.signature, Some(sig));
    }
}
