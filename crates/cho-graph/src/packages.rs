//! Which package (Cargo crate, npm package, Python project) a repository file belongs to.

use cho_semantic::relpath::parent;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageInfo {
    pub name: String,
    pub kind: PackageKind,
}

/// Which manifest names a package. Variants are in name order, as the strings sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PackageKind {
    Cargo,
    /// No manifest found.
    Dir,
    Npm,
    Python,
}

impl PackageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Dir => "dir",
            Self::Npm => "npm",
            Self::Python => "python",
        }
    }
}

/// Finds the nearest manifest above each file, caching per directory.
pub(crate) struct PackageDetector<'a> {
    root: &'a Path,
    by_dir: HashMap<String, Option<PackageInfo>>,
}

impl<'a> PackageDetector<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            by_dir: HashMap::new(),
        }
    }

    /// The package of `file` (repository-relative, `/`-separated). Files with no manifest
    /// above them belong to the package `(repo)`.
    pub fn package_of(&mut self, file: &str) -> PackageInfo {
        self.lookup(parent(file)).unwrap_or(PackageInfo {
            name: "(repo)".into(),
            kind: PackageKind::Dir,
        })
    }

    fn lookup(&mut self, dir: &str) -> Option<PackageInfo> {
        if let Some(hit) = self.by_dir.get(dir) {
            return hit.clone();
        }
        let found = read_manifest(self.root, dir).or_else(|| {
            (!dir.is_empty())
                .then(|| self.lookup(parent(dir)))
                .flatten()
        });
        self.by_dir.insert(dir.to_string(), found.clone());
        found
    }
}

/// A manifest's text: a regular file inside `root` (a symlink that leaves the repository reads
/// nothing) under the size limit (see `cho_core::fs`).
fn read_text(root: &Path, rel: &Path) -> Result<String, cho_core::fs::ReadError> {
    cho_core::fs::read_source_in(root, rel).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The package whose manifest is in `dir` (repository-relative, `/`-separated).
fn read_manifest(root: &Path, dir: &str) -> Option<PackageInfo> {
    let manifest = |name: &str| Path::new(dir).join(name);
    if let Ok(text) = read_text(root, &manifest("Cargo.toml"))
        && let Some(name) = toml_name(&text, "package")
    {
        return Some(PackageInfo {
            name,
            kind: PackageKind::Cargo,
        });
    }
    if let Ok(text) = read_text(root, &manifest("package.json"))
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(name) = v.get("name").and_then(|n| n.as_str())
    {
        return Some(PackageInfo {
            name: name.to_string(),
            kind: PackageKind::Npm,
        });
    }
    if let Ok(text) = read_text(root, &manifest("pyproject.toml"))
        && let Some(name) = toml_name(&text, "project")
    {
        return Some(PackageInfo {
            name,
            kind: PackageKind::Python,
        });
    }
    None
}

/// `name = "x"` inside `[table]`, without a TOML parser: manifests keep it on one line.
fn toml_name(text: &str, table: &str) -> Option<String> {
    let mut in_table = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_table = line == format!("[{table}]");
            continue;
        }
        if in_table && let Some(rest) = line.strip_prefix("name") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            let value = value.trim_matches(|c| c == '"' || c == '\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Cargo normalises `-` to `_` in crate names; SCIP symbols may use either.
pub fn same_package(a: &str, b: &str) -> bool {
    a == b || a.replace('-', "_") == b.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by the `graph_views` fuzz target: a manifest that is a symlink out of the
    /// repository was read, so a file elsewhere on disk could name a package.
    #[cfg(unix)]
    #[test]
    fn manifest_symlinked_out_of_the_repository_names_nothing() {
        let base = std::env::temp_dir().join(format!("cho-pkg-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("repo/crate")).unwrap();
        std::fs::write(base.join("outside.toml"), "[package]\nname = \"leaked\"\n").unwrap();
        std::os::unix::fs::symlink(
            base.join("outside.toml"),
            base.join("repo/crate/Cargo.toml"),
        )
        .unwrap();
        std::fs::write(
            base.join("repo/Cargo.toml"),
            "[package]\nname = \"inside\"\n",
        )
        .unwrap();
        let root = base.join("repo");
        let mut d = PackageDetector::new(&root);
        assert_eq!(d.package_of("crate/src/lib.rs").name, "inside");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn manifest_names() {
        assert_eq!(
            toml_name("[package]\nname = \"cho-core\"\nversion = \"1\"", "package").as_deref(),
            Some("cho-core")
        );
        assert_eq!(toml_name("[workspace]\nname = \"x\"", "package"), None);
    }
}
