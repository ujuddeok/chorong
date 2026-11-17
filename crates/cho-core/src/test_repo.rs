//! Running `git` in a scratch repository a test built (feature `test-support`; never in
//! product code, which uses [`crate::git::command`]).
//!
//! Hermetic: the user's global and system git config are ignored (no signing, hooks, template
//! folder or default branch name from the developer's machine), and the author, committer and
//! dates are fixed, so commit ids are the same on every machine. git is required to build and
//! test Chorong: a missing git fails the test with a clear message instead of passing it without
//! checking anything.

use std::path::Path;
use std::process::{Command, Output};

/// The commit date of every commit [`git`] makes.
pub const DATE: &str = "2026-01-01T00:00:00Z";

/// `git -C dir` with the hermetic environment, for a caller that needs the raw output.
pub fn command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Chorong Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_AUTHOR_DATE", DATE)
        .env("GIT_COMMITTER_NAME", "Chorong Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_DATE", DATE);
    cmd
}

/// Runs `git args` in `dir` and returns its output; panics when git cannot be started.
pub fn output(dir: &Path, args: &[&str]) -> Output {
    command(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git is required to run Chorong's tests; cannot run it: {e}"))
}

/// Runs `git args` in `dir` and returns its standard output, trimmed. Panics with git's error
/// output when the command fails.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = output(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `git init` on the branch `main`.
pub fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
}

/// Stages everything in `dir` and commits it (an empty commit when nothing changed); returns
/// the new commit id.
pub fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}
