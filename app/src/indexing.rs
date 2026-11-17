//! Building the semantic index from the viewer. Planning and running only; the window state
//! lives in `view.rs`. Uses the same `cho_semantic::indexer` functions as `cho index`, one
//! project at a time, and never runs a package install.

use cho_semantic::indexer::{self, Language, Project};
use cho_semantic::node_modules::{self, NodeModules};
use cho_semantic::{IndexCache, IndexStatus};
use std::path::Path;
use std::time::Duration;

/// Same limit as `cho index`'s default.
pub const TIMEOUT: Duration = Duration::from_secs(600);

/// One project to build, or the reason it is skipped.
#[derive(Debug, Clone)]
pub struct Item {
    pub project: Project,
    /// Cache directory name, e.g. `rust` or `typescript-packages_web`.
    pub label: String,
    /// Set when the project cannot be indexed now.
    pub blocked: Option<String>,
    /// A caution that does not stop the build.
    pub note: Option<String>,
    /// A workspace of many packages: the build can take minutes and gigabytes.
    pub large: bool,
}

/// Shown up front, in the status bar, while a large workspace builds. Never a modal.
pub const LARGE_NOTE: &str = "Large workspace: may take minutes and GBs of memory";

/// Is this TypeScript project a pnpm workspace or a package.json with `workspaces`?
fn is_workspace(project: &Project) -> bool {
    if project.language != Language::TypeScript {
        return false;
    }
    if project.root.join("pnpm-workspace.yaml").is_file() {
        return true;
    }
    cho_core::fs::read_source(&project.root.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v| v.get("workspaces").is_some())
}

/// Checks the TypeScript prerequisite (`cho_semantic::node_modules`, the rule `cho index`
/// uses too). Without an installed `node_modules` the project still builds, with a caution:
/// imports between packages and from dependencies don't resolve until the reader installs
/// (design/manual-start.md, decision 15). Chorong never installs them.
fn node_modules_check(project: &Project, repo_root: &Path) -> (Option<String>, Option<String>) {
    match node_modules::check(project, repo_root) {
        NodeModules::Installed => (None, None),
        NodeModules::Missing { at } => (
            None,
            Some(format!(
                "no node_modules in {at}: imports between packages won't resolve until you install the dependencies (Chorong never installs)"
            )),
        ),
        NodeModules::NotNeeded { at } => (
            None,
            Some(format!(
                "no node_modules in {at}; the project declares no dependencies"
            )),
        ),
    }
}

/// Everything under `root` that can be indexed, in build order, with what each needs.
pub fn plan(root: &Path) -> Vec<Item> {
    indexer::detect(root, None)
        .into_iter()
        .map(|project| {
            let (blocked, note) = if project.language == Language::TypeScript {
                node_modules_check(&project, root)
            } else {
                (None, None)
            };
            Item {
                label: project.slug(),
                large: is_workspace(&project),
                project,
                blocked,
                note,
            }
        })
        .collect()
}

/// The items that need building: no current index for them in `cache`.
pub fn needs_build(cache: &IndexCache, root: &Path, items: Vec<Item>) -> Vec<Item> {
    let existing = cache.projects(root);
    items
        .into_iter()
        .filter(|i| {
            !existing.iter().any(|e| {
                e.dir
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy() == i.label)
                    && e.status == IndexStatus::Current
                    && e.has_index
            })
        })
        .collect()
}

/// What a finished build did, for the status bar.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub built: usize,
    /// `label: first line of the problem` for each failure or skipped project.
    pub problems: Vec<String>,
    /// Cautions that did not stop a build, e.g. TypeScript without `node_modules`.
    pub notes: Vec<String>,
}

/// Does the repository hold anything the viewer could index?
pub fn has_projects(root: &Path) -> bool {
    !indexer::detect(root, None).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
        let guard = tempfile::tempdir().unwrap();
        let d = guard.path().to_path_buf();
        (guard, d)
    }

    #[test]
    fn typescript_with_dependencies_and_no_node_modules_builds_with_a_warning() {
        let (_guard, d) = tmp();
        std::fs::write(
            d.join("package.json"),
            r#"{"dependencies":{"left-pad":"1"}}"#,
        )
        .unwrap();
        let items = plan(&d);
        assert_eq!(items.len(), 1);
        assert!(items[0].blocked.is_none());
        let why = items[0].note.as_deref().unwrap();
        assert!(
            why.contains("won't resolve") && why.contains("never installs"),
            "{why}"
        );
        std::fs::create_dir(d.join("node_modules")).unwrap();
        assert!(plan(&d)[0].note.is_none());
    }

    #[test]
    fn typescript_without_dependencies_builds_with_a_note() {
        let (_guard, d) = tmp();
        std::fs::write(
            d.join("package.json"),
            r#"{"name":"x","workspaces":["p/*"]}"#,
        )
        .unwrap();
        let items = plan(&d);
        assert!(items[0].blocked.is_none());
        assert!(items[0].note.is_some());
    }

    #[test]
    fn workspaces_are_large_and_plain_packages_are_not() {
        let (_guard, d) = tmp();
        std::fs::write(d.join("package.json"), r#"{"name":"x"}"#).unwrap();
        assert!(!plan(&d)[0].large);
        std::fs::write(d.join("package.json"), r#"{"workspaces":["p/*"]}"#).unwrap();
        assert!(plan(&d)[0].large);
        std::fs::write(d.join("package.json"), r#"{"name":"x"}"#).unwrap();
        std::fs::write(d.join("pnpm-workspace.yaml"), "packages:\n  - p/*\n").unwrap();
        assert!(plan(&d)[0].large);
    }

    #[test]
    fn a_folder_with_no_project_has_nothing_to_build() {
        let (_guard, d) = tmp();
        assert!(plan(&d).is_empty());
        assert!(!has_projects(&d));
    }
}
