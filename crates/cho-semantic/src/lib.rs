//! Semantic navigation from SCIP indexes: go to definition, references and
//! symbol search, plus the indexer runners and cache that produce them.
//! See `docs/public/semantic.md`.
//!
//! Nothing here guesses. A symbol comes from an occurrence in an index, and
//! positions become byte offsets only through `cho_core::SourceBuffer`.

pub mod cache;
pub mod dist_link;
pub mod hover;
pub mod index;
pub mod indexer;
pub mod node_modules;
mod peek;
pub mod progress;
pub mod query;
pub mod relpath;
pub mod symbol;
pub mod tools;
pub mod workspace;

pub use cache::{
    BuildResult, Failure, GitState, IndexCache, IndexStatus, Meta, PrepareFailure, PrepareNote,
    ProjectEntry,
};
pub use dist_link::{DistLinks, LinkStats, MapMethod, Mapping};
pub use hover::{
    DeclarationKind, DeclarationPeek, Hover, HoverDefinition, HoverIndex, HoverSignature,
    NoDeclaration, NoSignature, SignatureSource, symbol_name,
};
pub use index::{
    Document, Encoding, Occurrence, RawRange, ScipIndex, Signature, SymbolId, SymbolInfo,
};
pub use indexer::{Language, Project};
pub use query::{DocOccurrence, IndexOrigin, Location, MappedFrom, Semantic, SymbolAt, SymbolHit};

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SemanticError {
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot decode SCIP index: {0}")]
    Decode(String),
}

impl Semantic {
    /// Loads every cached project index for the repository at `root`. `None`
    /// if there are none. A project whose last run failed is still loaded if
    /// an earlier run left an index file; [`Semantic::status`] says `failed`.
    pub fn open(cache: &IndexCache, root: &Path) -> Result<Option<Semantic>, SemanticError> {
        let root = root.canonicalize().map_err(|e| SemanticError::Io {
            path: root.to_path_buf(),
            source: e,
        })?;
        let projects = cache.projects(&root);
        let mut merged: Option<ScipIndex> = None;
        let mut origins = vec![];
        for p in projects.iter().filter(|p| p.has_index) {
            let loaded = ScipIndex::load(&p.index_path(), &p.meta.project_root)?;
            origins.push(IndexOrigin {
                project_root: relpath::trim_root(&p.meta.project_root).to_string(),
                commit: p.meta.git_head.clone(),
                status: p.status,
            });
            match &mut merged {
                Some(m) => m.merge(loaded),
                None => merged = Some(loaded),
            }
        }
        let status = cache::overall_status(&projects);
        Ok(merged.map(|index| {
            Semantic::new(index, root, status)
                .with_origins(origins)
                .with_dist_links()
        }))
    }
}
