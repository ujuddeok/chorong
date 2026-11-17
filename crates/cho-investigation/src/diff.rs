//! Diff panes (format version 2): one file between two revisions, `base` and `head` (the
//! working tree when absent). Revisions come from untrusted files, so a name is checked
//! before git sees it, git runs only through `cho_core::git::command`, and the name goes
//! after `--end-of-options`, never as an option. Nothing here guesses: a name git can't
//! resolve is reported as unresolved.

use crate::drift::CommitReader;
use serde::Serialize;
use std::path::Path;

/// Longest revision name accepted.
const MAX_REV: usize = 255;

/// Whether `rev` may name a branch, tag or commit in a diff pane: not empty, not an option
/// (`-…`), no whitespace or control characters, no range (`..`), path (`:`) or reflog
/// (`@{`) syntax. `HEAD~2` and `v1.2^` are fine.
pub fn is_rev_name(rev: &str) -> bool {
    !rev.is_empty()
        && rev.len() <= MAX_REV
        && !rev.starts_with('-')
        && !rev.contains("..")
        && !rev.contains("@{")
        && !rev.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, ':' | '\\' | '?' | '*' | '[')
        })
}

/// The full commit `rev` names in the repository at `root`, or why not.
pub fn resolve(root: &Path, rev: &str) -> Result<String, String> {
    if !is_rev_name(rev) {
        return Err(format!("`{rev}` is not a branch, tag or commit name"));
    }
    crate::drift::commit_of(root, rev)?
        .ok_or_else(|| format!("`{rev}` is not a commit in this repository"))
}

/// One side of a diff pane, resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevReport {
    /// As written; `None` for the working tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// The full commit, when it resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Whether the file is there on this side.
    pub has_file: bool,
    /// Why the revision or the file could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RevReport {
    /// `main`, or "the working tree".
    pub fn words(&self) -> String {
        self.rev
            .clone()
            .unwrap_or_else(|| "the working tree".to_string())
    }
}

/// Both sides of a diff pane with the file's bytes on each (`None` where it is absent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTexts {
    pub base: RevReport,
    pub head: RevReport,
    pub base_bytes: Option<Vec<u8>>,
    pub head_bytes: Option<Vec<u8>>,
}

impl DiffTexts {
    /// Why the pane can't show a diff: an unresolved revision, or the file on neither side.
    pub fn problem(&self, path: &str) -> Option<String> {
        if let Some(e) = self
            .base
            .error
            .as_ref()
            .filter(|_| self.base.commit.is_none())
        {
            return Some(format!("base {e}"));
        }
        if let Some(e) = &self.head.error
            && self.head.rev.is_some()
            && self.head.commit.is_none()
        {
            return Some(format!("head {e}"));
        }
        if let Some(e) = self.base.error.as_ref().or(self.head.error.as_ref()) {
            return Some(e.clone());
        }
        if !self.base.has_file && !self.head.has_file {
            return Some(format!(
                "`{path}` is in neither {} nor {}",
                self.base.words(),
                self.head.words()
            ));
        }
        None
    }
}

/// Reads `path` at `base` and at `head` (the working tree when `None`) in the repository at
/// `root`. Never fails: what could not be resolved or read is in the sides' `error`.
pub fn read(root: &Path, path: &str, base: &str, head: Option<&str>) -> DiffTexts {
    let at_rev = |rev: &str| -> (RevReport, Option<Vec<u8>>) {
        let mut r = RevReport {
            rev: Some(rev.to_string()),
            commit: None,
            has_file: false,
            error: None,
        };
        let commit = match resolve(root, rev) {
            Ok(c) => c,
            Err(e) => {
                r.error = Some(e);
                return (r, None);
            }
        };
        r.commit = Some(commit.clone());
        let bytes = CommitReader::new(root, &commit).and_then(|c| c.read_blob(path));
        match bytes {
            Ok(b) => {
                r.has_file = b.is_some();
                (r, b)
            }
            Err(e) => {
                r.error = Some(e);
                (r, None)
            }
        }
    };
    let (base_r, base_bytes) = at_rev(base);
    let (head_r, head_bytes) = match head {
        Some(h) => at_rev(h),
        None => {
            let mut r = RevReport {
                rev: None,
                commit: None,
                has_file: false,
                error: None,
            };
            let bytes = if root.join(path).exists() {
                match cho_core::fs::read_source_in(root, Path::new(path)) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        r.error = Some(format!("`{path}`: {e}"));
                        None
                    }
                }
            } else {
                None
            };
            r.has_file = bytes.is_some();
            (r, bytes)
        }
    };
    DiffTexts {
        base: base_r,
        head: head_r,
        base_bytes,
        head_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_core::test_repo::git as sh;

    #[test]
    fn revision_names_never_reach_git_as_options_or_ranges() {
        for ok in [
            "main",
            "feature/orders",
            "v1.2",
            "HEAD~2",
            "3f9c1e2",
            "HEAD^",
        ] {
            assert!(is_rev_name(ok), "{ok}");
        }
        for bad in [
            "",
            "-n",
            "--output=x",
            "a..b",
            "a...b",
            "main:src/a.rs",
            "a b",
            "a\nb",
            "@{-1}",
            "x*",
        ] {
            assert!(!is_rev_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn reads_both_sides_and_reports_what_is_missing() {
        let dir = std::env::temp_dir().join(format!("cho-inv-diff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        sh(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "one"]);
        sh(&dir, &["switch", "-q", "-c", "feature"]);
        std::fs::write(dir.join("src/a.rs"), "fn a() { 1; }\n").unwrap();
        std::fs::write(dir.join("src/b.rs"), "fn b() {}\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-q", "-m", "two"]);
        std::fs::write(dir.join("src/a.rs"), "fn a() { 2; }\n").unwrap();

        let d = read(&dir, "src/a.rs", "main", Some("feature"));
        assert_eq!(d.problem("src/a.rs"), None);
        assert_eq!(d.base_bytes.as_deref(), Some(&b"fn a() {}\n"[..]));
        assert_eq!(d.head_bytes.as_deref(), Some(&b"fn a() { 1; }\n"[..]));
        // No head: the working tree.
        let d = read(&dir, "src/a.rs", "main", None);
        assert_eq!(d.head_bytes.as_deref(), Some(&b"fn a() { 2; }\n"[..]));
        // A new file has no base, and that is fine.
        let d = read(&dir, "src/b.rs", "main", None);
        assert!(!d.base.has_file && d.head.has_file);
        assert_eq!(d.problem("src/b.rs"), None);
        // Not on either side.
        let d = read(&dir, "src/zz.rs", "main", None);
        assert!(
            d.problem("src/zz.rs")
                .unwrap()
                .contains("neither main nor the working tree")
        );
        // An unknown base is reported, never guessed.
        let d = read(&dir, "src/a.rs", "nope", None);
        assert!(d.base.commit.is_none());
        assert!(d.problem("src/a.rs").unwrap().starts_with("base `nope`"));
        let d = read(&dir, "src/a.rs", "main", Some("--all"));
        assert!(d.problem("src/a.rs").unwrap().starts_with("head"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
