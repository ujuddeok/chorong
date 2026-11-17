//! Which paths differ between `HEAD` and the working folder, read with `gix` (no git
//! subprocess, nothing written: the index is only read).

use std::path::Path;

/// Repository-relative, `/`-separated paths that may differ between `HEAD` and the working
/// folder: files changed in the working folder or staged, and untracked files (gitignored files
/// and submodule contents are not listed). Sorted. A path can be listed although its contents
/// equal `HEAD`'s (a file touched without a change); callers compare contents.
pub fn uncommitted_paths(root: &Path) -> Result<Vec<String>, String> {
    use gix::status::{UntrackedFiles, tree_index::TrackRenames};
    let repo = gix::open(root).map_err(|e| format!("cannot open the repository: {e}"))?;
    let iter = repo
        .status(gix::progress::Discard)
        .map_err(|e| format!("cannot read the working folder status: {e}"))?
        .untracked_files(UntrackedFiles::Files)
        .index_worktree_rewrites(None)
        .index_worktree_submodules(None)
        .tree_index_track_renames(TrackRenames::Disabled)
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|e| format!("cannot read the working folder status: {e}"))?;
    let mut out = Vec::new();
    for item in iter {
        let item = item.map_err(|e| format!("cannot read the working folder status: {e}"))?;
        let path = match &item {
            gix::status::Item::IndexWorktree(i) => i.rela_path().to_string(),
            gix::status::Item::TreeIndex(c) => c.location().to_string(),
        };
        out.push(path);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use cho_core::test_repo::git as sh;

    #[test]
    fn lists_edited_staged_deleted_and_untracked_files_but_not_ignored_ones() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        sh(d, &["init", "-q", "-b", "main"]);
        std::fs::create_dir(d.join("src")).unwrap();
        for (p, t) in [
            (".gitignore", "target/\n"),
            ("src/a.rs", "1\n"),
            ("src/b.rs", "1\n"),
            ("src/c.rs", "1\n"),
            ("src/d.rs", "1\n"),
        ] {
            std::fs::write(d.join(p), t).unwrap();
        }
        sh(d, &["add", "."]);
        sh(d, &["commit", "-q", "-m", "one"]);
        assert_eq!(uncommitted_paths(d).unwrap(), Vec::<String>::new());
        std::fs::write(d.join("src/a.rs"), "2\n").unwrap();
        std::fs::write(d.join("src/b.rs"), "2\n").unwrap();
        sh(d, &["add", "src/b.rs"]); // staged, working folder equals the index
        std::fs::remove_file(d.join("src/c.rs")).unwrap();
        std::fs::create_dir_all(d.join("src/new")).unwrap();
        std::fs::write(d.join("src/new/e.rs"), "x\n").unwrap();
        std::fs::write(d.join("src/new/f.rs"), "x\n").unwrap();
        std::fs::create_dir(d.join("target")).unwrap();
        std::fs::write(d.join("target/out.o"), "x\n").unwrap();
        assert_eq!(
            uncommitted_paths(d).unwrap(),
            [
                "src/a.rs",
                "src/b.rs",
                "src/c.rs",
                "src/new/e.rs",
                "src/new/f.rs"
            ]
        );
        assert!(uncommitted_paths(&d.join("missing")).is_err());
    }
}
