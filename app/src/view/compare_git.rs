//! The git facts "Compare against…" needs, as a thin layer over `cho_git::ObjectStore` (merge
//! base, changed files with renames, blobs) and `cho_git::linediff`. Nothing here reads or
//! writes the index or working tree of the user's repository; the "after" side is the file
//! read from disk by the caller.

use cho_git::{LineHunk, ObjectStore};
use std::path::{Path, PathBuf};

/// The merge base of `HEAD` and `against`: `(against as a full commit, base, number of merge
/// bases)`. With several bases the newest committer time wins, ties go to the lowest id.
pub(crate) fn merge_base(root: &Path, against: &str) -> Result<(String, String, usize), String> {
    let store = ObjectStore::discover(root)?;
    let full = store.commit_id(against)?;
    let mb = store
        .merge_base("HEAD", &full)?
        .ok_or_else(|| format!("HEAD and `{against}` have no common history"))?;
    Ok((full, mb.chosen, mb.all.len()))
}

/// One differing region between a file at the merge base and in the working folder, as
/// zero-based, half-open lines, and whether uncommitted work touches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileHunk {
    pub hunk: LineHunk,
    pub uncommitted: bool,
}

/// A file as the merge base has it, and how the working-folder file differs from it.
#[derive(Debug, Clone)]
pub(crate) struct FileDiff {
    /// The repository's top level (git paths are relative to it).
    pub top: PathBuf,
    /// The path at the base (follows renames), `None` when the file is new.
    pub path: Option<String>,
    /// The bytes at the base, `None` when the file is new.
    pub bytes: Option<Vec<u8>>,
    /// Base to working folder, cut by what is committed (`cho_changes`).
    pub hunks: Vec<FileHunk>,
    /// The line diff hit its cost limit: one block.
    pub coarse: bool,
}

/// The bytes of `rel` (relative to the repository's top) at the commit `base`.
pub(crate) fn base_blob(root: &Path, base: &str, rel: &str) -> Result<Vec<u8>, String> {
    ObjectStore::discover(root)?
        .blob_at(base, rel)?
        .ok_or_else(|| format!("{rel} is not in the merge base"))
}

/// The repository's working folder containing `path`, found as git does (gix).
pub(crate) fn repo_top(path: &Path) -> Result<PathBuf, String> {
    ObjectStore::discover(path)?
        .workdir()
        .ok_or_else(|| "this repository has no working folder".to_string())
}

/// The merge-base version of `file` (an absolute path in the working folder) and the hunks
/// from it to the file as it is on disk, committed and uncommitted told apart. A file renamed
/// since the base is read from its old path; one the base does not have has no bytes.
pub(crate) fn file_diff(root: &Path, base: &str, file: &Path) -> Result<FileDiff, String> {
    let store = ObjectStore::discover(root)?;
    let top = store
        .workdir()
        .ok_or("this repository has no working folder")?;
    let rel = rel_path(&top, file).ok_or("this file is outside the repository")?;
    let changes = cho_changes::worktree_file_changes(&top, base, &rel)?;
    let base_path = match &changes {
        Some(c) => c.path_at(cho_changes::Side::Base).map(str::to_string),
        None => Some(rel.clone()),
    };
    let bytes = match &base_path {
        Some(p) => store.blob_at(base, p)?,
        None => None,
    };
    let (hunks, coarse) = changes.map_or((Vec::new(), false), |c| {
        let hunks = c
            .hunks
            .iter()
            .map(|h| FileHunk {
                hunk: LineHunk {
                    base: h.base.start_line - 1..h.base.start_line - 1 + h.base.line_count,
                    head: h.head.start_line - 1..h.head.start_line - 1 + h.head.line_count,
                },
                uncommitted: h.uncommitted,
            })
            .collect();
        (hunks, c.coarse)
    });
    Ok(FileDiff {
        top,
        path: bytes.as_ref().and(base_path),
        bytes,
        hunks,
        coarse,
    })
}

fn rel_path(top: &Path, file: &Path) -> Option<String> {
    let top = top.canonicalize().ok()?;
    let file = file.canonicalize().ok()?;
    let rel = file.strip_prefix(&top).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use cho_core::test_repo::git as sh;

    /// The viewer opened on a subfolder (or on one file in it): the repository's top is above
    /// the opened root, and every adapter call still finds it and keeps git paths top-relative.
    #[test]
    fn comparing_works_from_a_subfolder_of_the_repository() {
        // git is required to build and test Chorong: a missing git fails here rather than
        // passing without checking anything.
        let dir = std::env::temp_dir().join(format!("cho-cmp-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("pkg/src");
        std::fs::create_dir_all(&sub).unwrap();
        sh(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(sub.join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "one"]);
        sh(&dir, &["switch", "-q", "-c", "feature"]);
        std::fs::write(sub.join("a.rs"), "fn a() {}\nfn b() { 1; }\n").unwrap();
        sh(&dir, &["commit", "-q", "-am", "two"]);

        let top = repo_top(&sub).unwrap();
        assert_eq!(top.canonicalize().unwrap(), dir.canonicalize().unwrap());
        let (_, base, _) = merge_base(&sub, "main").unwrap();
        let diff = file_diff(&sub, &base, &sub.join("a.rs")).unwrap();
        assert_eq!(diff.path.as_deref(), Some("pkg/src/a.rs"));
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(
            base_blob(&sub, &base, "pkg/src/a.rs").unwrap(),
            b"fn a() {}\nfn b() {}\n"
        );
        // The Changes list runs from the top found above the opened root.
        let set = cho_changes::compare_worktree(&top, &base).unwrap();
        assert_eq!(set.files.len(), 1);
        assert_eq!(set.files[0].path, "pkg/src/a.rs");
        assert_eq!(cho_git::list_refs(&sub).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
