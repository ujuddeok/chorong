//! Definition peek: the full byte range of the declaration a definition belongs to.
//!
//! The range comes from the Tree-sitter outline of the defining file (`cho-syntax`):
//! the innermost declaration whose name range contains the definition occurrence,
//! or, for fields, parameters and locals, the innermost declaration containing it.
//! Parsing costs real time, so outlines are cached per file, keyed by length and
//! modification time, in a small LRU. Nothing here runs unless a caller asks.

use crate::hover::{DeclarationPeek, NoDeclaration};
use crate::query::{Location, Semantic};
use cho_core::SourceBuffer;
use cho_syntax::{Language, Symbol, outline};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

/// How many files' outlines are kept.
const CAPACITY: usize = 64;

struct Entry {
    path: String,
    len: u64,
    mtime: Option<SystemTime>,
    outline: Arc<Vec<Symbol>>,
}

/// Least-recently-used outlines; the front is the most recent.
#[derive(Default)]
pub(crate) struct OutlineCache {
    entries: Vec<Entry>,
}

impl OutlineCache {
    fn get(&mut self, path: &str, len: u64, mtime: Option<SystemTime>) -> Option<Arc<Vec<Symbol>>> {
        let i = self
            .entries
            .iter()
            .position(|e| e.path == path && e.len == len && e.mtime == mtime)?;
        let e = self.entries.remove(i);
        let hit = e.outline.clone();
        self.entries.insert(0, e);
        Some(hit)
    }

    fn put(&mut self, entry: Entry) {
        self.entries.retain(|e| e.path != entry.path);
        self.entries.insert(0, entry);
        self.entries.truncate(CAPACITY);
    }
}

impl Semantic {
    /// What to show for `definition`: its own declaration (the innermost outline
    /// declaration whose name range contains it), else the innermost declaration that
    /// contains it (a field's struct, a parameter's or local's function) with the
    /// member's lines marked. Parses the file on a cache miss (milliseconds for a
    /// large file) and is a lookup otherwise; it is meant for an explicit request,
    /// not for every pointer move, and not for the UI thread.
    pub fn peek_definition(&self, definition: &Location) -> Result<DeclarationPeek, NoDeclaration> {
        let path = definition.path.as_str();
        if !crate::relpath::is_safe_relative(path) {
            return Err(NoDeclaration::OutsideIndexedFiles);
        }
        let rel = Path::new(path);
        let full = self.root().join(rel);
        let meta = std::fs::metadata(&full).map_err(|_| NoDeclaration::OutsideIndexedFiles)?;
        if !meta.is_file() {
            return Err(NoDeclaration::OutsideIndexedFiles);
        }
        let language = Language::from_path(rel).ok_or(NoDeclaration::UnsupportedLanguage)?;
        let (len, mtime) = (meta.len(), meta.modified().ok());
        let cached = self
            .outlines
            .lock()
            .ok()
            .and_then(|mut c| c.get(path, len, mtime));
        let symbols = match cached {
            Some(s) => s,
            None => {
                let bytes = cho_core::fs::read_source_in(self.root(), rel)
                    .map_err(|_| NoDeclaration::OutsideIndexedFiles)?;
                let buffer = SourceBuffer::from_bytes(bytes)
                    .map_err(|_| NoDeclaration::OutsideIndexedFiles)?;
                let symbols = Arc::new(outline(language, buffer.text()));
                if let Ok(mut c) = self.outlines.lock() {
                    c.put(Entry {
                        path: path.to_string(),
                        len,
                        mtime,
                        outline: symbols.clone(),
                    });
                }
                symbols
            }
        };
        let at = &definition.range;
        if let Some(own) = symbols
            .iter()
            .filter(|s| s.name_range.start <= at.start && at.end <= s.name_range.end)
            .min_by_key(|s| s.range.len())
        {
            return Ok(DeclarationPeek::Declaration {
                range: own.range.clone(),
            });
        }
        let enclosing = symbols
            .iter()
            .filter(|s| s.range.start <= at.start && at.end <= s.range.end)
            .min_by_key(|s| s.range.len())
            .ok_or(NoDeclaration::NoDeclaration)?;
        // The member's own lines: those the definition occurrence covers, kept
        // inside the enclosing declaration. Needs the file's current text.
        let buffer = self
            .buffer(path)
            .ok_or(NoDeclaration::OutsideIndexedFiles)?;
        let first = buffer.line_range(buffer.line_of(at.start));
        let last = buffer.line_range(buffer.line_of(at.end.saturating_sub(1).max(at.start)));
        let marked = match (first, last) {
            (Some(a), Some(b)) => {
                a.start.max(enclosing.range.start)..b.end.min(enclosing.range.end)
            }
            _ => at.clone(),
        };
        Ok(DeclarationPeek::Enclosing {
            range: enclosing.range.clone(),
            marked,
        })
    }
}
