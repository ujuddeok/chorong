//! Listing a folder's files for the sidebar and picker.
//!
//! Uses ripgrep's walker (the `ignore` crate), so `.gitignore`, `.ignore` and global Git
//! excludes are respected, hidden files are skipped, and generated output such as
//! `target/` or fuzz corpora never shows up. Works the same outside a Git repository.

use std::path::{Path, PathBuf};

const MAX_FILES: usize = 50_000;

/// Repository-relative paths, sorted so each folder's files group together.
pub fn list_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        // Respect .gitignore even when the folder isn't inside a Git repository.
        .require_git(false)
        .build();
    for entry in walker.flatten() {
        if out.len() >= MAX_FILES {
            break;
        }
        if entry.file_type().is_some_and(|t| t.is_file())
            && let Ok(rel) = entry.path().strip_prefix(root)
        {
            out.push(rel.to_path_buf());
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn respects_gitignore_and_hidden_files() {
        let dir_guard = tempfile::tempdir().unwrap();
        let dir = dir_guard.path().to_path_buf();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::create_dir_all(dir.join("fuzz/corpus/x")).unwrap();
        fs::write(dir.join(".gitignore"), "/target\n/fuzz/corpus\n").unwrap();
        fs::write(dir.join("src/lib.rs"), "").unwrap();
        fs::write(dir.join("target/debug/out"), "").unwrap();
        fs::write(dir.join("fuzz/corpus/x/1"), "").unwrap();
        fs::write(dir.join(".secret"), "").unwrap();
        let files = list_files(&dir);
        assert_eq!(files, vec![PathBuf::from("src/lib.rs")]);
    }
}
