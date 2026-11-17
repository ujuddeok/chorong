//! How the folders the viewer has opened are listed. The list itself lives in
//! `cho_sources::recent` (one `recent.json`, one writer, shared with `cho open`); this module
//! only names its entries: a worktree in Chorong's repository cache reads as
//! `github:acme/api · pull request 12`, any other folder as its path.

use cho_sources::Cache;
use cho_sources::state::RepoState;
use std::path::Path;

/// The label of a recent folder. `cache` is Chorong's repository cache, when it has a location.
pub fn label(folder: &Path, cache: Option<&Cache>) -> String {
    let Some(cache) = cache else {
        return folder.display().to_string();
    };
    let Some((repo, slug)) = cache.worktree_owner(folder) else {
        return folder.display().to_string();
    };
    let what = cache
        .repo(&repo)
        .ok()
        .and_then(|p| {
            RepoState::load(&p.state)
                .worktrees
                .get(&slug)
                .map(|w| w.what.clone())
        })
        .filter(|w| !w.is_empty())
        .unwrap_or(slug);
    format!("{repo} \u{b7} {what}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_core::RepoRef;
    use cho_sources::state::WorktreeState;

    fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
        let guard = tempfile::tempdir().unwrap();
        let dir = guard.path().to_path_buf();
        (guard, dir)
    }

    #[test]
    fn cache_worktrees_read_as_repository_and_what() {
        let (_guard, dir) = scratch();
        let cache = Cache::at(&dir);
        let repo = RepoRef::parse("github:acme/api").unwrap();
        let paths = cache.repo(&repo).unwrap();
        let wt = paths.worktree("pr-12");
        std::fs::create_dir_all(&wt).unwrap();
        let mut st = RepoState::default();
        st.touch(
            "pr-12",
            WorktreeState {
                what: "pull request 12".into(),
                ..Default::default()
            },
        );
        st.save(&paths.state).unwrap();
        assert_eq!(
            label(&wt, Some(&cache)),
            "github:acme/api \u{b7} pull request 12"
        );
        // No state entry: the folder name stands in.
        let other = paths.worktree("feat-x");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(label(&other, Some(&cache)), "github:acme/api \u{b7} feat-x");
    }

    #[test]
    fn other_folders_read_as_their_path() {
        let (_guard, dir) = scratch();
        let cache = Cache::at(dir.join("cache"));
        assert_eq!(
            label(Path::new("/work/chorong"), Some(&cache)),
            "/work/chorong"
        );
        assert_eq!(label(Path::new("/work/chorong"), None), "/work/chorong");
    }
}
