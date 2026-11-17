//! Call kinds for the Critical Lens, from the SCIP symbol at each call token.
//!
//! - **Repo**: the definition is in the repository (a local symbol, or a symbol whose
//!   package is one of the project's own packages).
//! - **External**: a SCIP symbol whose package is not a project package, so it is defined
//!   in a dependency.
//! - **Unknown**: the index has no symbol at that position, or there is no index.
//!
//! A project package is any (scheme, manager, name) that has a definition occurrence in the
//! index: indexers emit definitions only for code they were asked to index. Nothing is
//! guessed from names.

use cho_semantic::{Semantic, index::ScipIndex};
use std::collections::HashSet;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Repo,
    External,
    Unknown,
}

/// The packages the indexed project itself defines.
#[derive(Debug, Default)]
pub struct ProjectPackages(HashSet<String>);

/// `scheme manager name` of a global SCIP symbol (`scheme manager name version descriptors`).
/// `None` for local symbols and malformed ones.
pub fn package_of(symbol: &str) -> Option<String> {
    if symbol.starts_with("local ") {
        return None;
    }
    let mut parts = symbol.splitn(5, ' ');
    let (scheme, manager, name) = (parts.next()?, parts.next()?, parts.next()?);
    parts.next()?; // version
    parts.next()?; // descriptors
    Some(format!("{scheme} {manager} {name}"))
}

impl ProjectPackages {
    pub fn from_index(index: &ScipIndex) -> Self {
        let mut set = HashSet::new();
        for (id, occ) in index.symbols() {
            if !occ.definitions.is_empty()
                && let Some(p) = package_of(&id.symbol)
            {
                set.insert(p);
            }
        }
        Self(set)
    }

    #[cfg(test)]
    pub fn from_packages(p: &[&str]) -> Self {
        Self(p.iter().map(|s| s.to_string()).collect())
    }

    /// Classifies a symbol the index found at a call token.
    pub fn classify(&self, symbol: &str) -> CallKind {
        if symbol.starts_with("local ") {
            return CallKind::Repo;
        }
        match package_of(symbol) {
            Some(p) if self.0.contains(&p) => CallKind::Repo,
            Some(_) => CallKind::External,
            // Not a well-formed global symbol: do not claim anything about it.
            None => CallKind::Unknown,
        }
    }
}

/// One kind per call range of `path`, looked up in the index. Runs off the UI thread.
pub fn classify_calls(
    sem: &Semantic,
    packages: &ProjectPackages,
    path: &str,
    calls: &[Range<usize>],
) -> Vec<CallKind> {
    calls
        .iter()
        .map(|c| match sem.symbol_at(path, c.start) {
            Some(at) => packages.classify(&at.id.symbol),
            None => CallKind::Unknown,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_are_scheme_manager_name() {
        assert_eq!(
            package_of("rust-analyzer cargo cho-core 0.1.0 buffer/SourceBuffer#line_of()."),
            Some("rust-analyzer cargo cho-core".into())
        );
        assert_eq!(package_of("local 3"), None);
        assert_eq!(package_of("garbage"), None);
    }

    #[test]
    fn project_symbols_are_repo_dependencies_external() {
        let p = ProjectPackages::from_packages(&["rust-analyzer cargo cho-core"]);
        assert_eq!(
            p.classify("rust-analyzer cargo cho-core 0.1.0 a/b()."),
            CallKind::Repo
        );
        assert_eq!(
            p.classify("rust-analyzer cargo std 1.0 vec/Vec#push()."),
            CallKind::External
        );
        assert_eq!(p.classify("local 7"), CallKind::Repo);
        assert_eq!(p.classify("nonsense"), CallKind::Unknown);
    }
}
