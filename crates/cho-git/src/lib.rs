//! Read-only git facts for Chorong: which repository a worktree is (by its remote), its branch,
//! commit and dirty state, its sibling worktrees, the hunks of a diff, a file's bytes at
//! another revision, who last changed each line (`git blame`), and the files and lines that differ between two commits.
//!
//! Nothing here writes to a repository or moves a checkout. Status, diffs and worktree lists
//! come from read-only `git` commands; file contents at a revision, tree compares and merge
//! bases are read from the object store with `gix` ([`ObjectStore`]), and line diffs are
//! computed in process ([`linediff`], and changed words within paired lines with
//! [`worddiff`]). See `docs/public/url-scheme.md` ("Repository identity in generated links",
//! "Commit mismatch").

pub mod blame;
pub mod linediff;
mod refs;
mod remote;
mod status;
mod tree;
pub mod worddiff;

pub use blame::{Blame, BlameCommit, parse_porcelain};
pub use linediff::{LineDiff, LineHunk, Lines, diff_lines};
pub use refs::{PreferredRefs, RefEntry, RefKind, list_refs, preferred_refs};
pub use remote::repo_ref_for_remote;
pub use status::uncommitted_paths;
pub use tree::{
    EntryKind, FileChange, FileSide, FileStatus, MergeBase, ObjectStore, RENAME_PAIR_LIMIT,
    RENAME_THRESHOLD, TreeChanges, is_binary,
};

use cho_core::RepoRef;
use std::path::{Path, PathBuf};

/// Runs a read-only git command in `root`; `None` when git is missing or the command fails.
/// The repository's own config cannot make it run programs, and `GIT_OPTIONAL_LOCKS=0` stops
/// commands such as `git status` from rewriting `.git/index` or taking `index.lock` (see
/// [`cho_core::git`]).
pub fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = cho_core::git::command(root).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Where a worktree is: branch, commit and whether it has uncommitted changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitInfo {
    pub branch: Option<String>,
    /// Abbreviated (7 characters).
    pub commit: Option<String>,
    pub full_commit: Option<String>,
    pub dirty: bool,
}

pub fn info(root: &Path) -> GitInfo {
    let full_commit = git(root, &["rev-parse", "HEAD"]);
    let branch = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]);
    GitInfo {
        commit: full_commit.as_ref().map(|c| short(c)),
        full_commit,
        branch,
        dirty: git(root, &["status", "--porcelain", "--ignore-submodules=all"])
            .is_some_and(|s| !s.is_empty()),
    }
}

/// The first seven characters of a commit id.
pub use cho_core::words::short_commit as short;

/// The top of the working tree `root` is in, or `root` itself when git can't say.
pub fn toplevel(root: &Path) -> PathBuf {
    git(root, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_path_buf())
}

/// The full commit id `rev` names, if it is a commit in this repository.
pub fn rev_parse(root: &Path, rev: &str) -> Option<String> {
    git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
}

/// The URL of the remote that names this repository: `origin`, else the only remote.
pub fn identity_remote(root: &Path) -> Option<String> {
    let names = git(root, &["remote"])?;
    let names: Vec<&str> = names.lines().filter(|l| !l.is_empty()).collect();
    let name = if names.contains(&"origin") {
        "origin"
    } else if names.len() == 1 {
        names[0]
    } else {
        return None;
    };
    git(root, &["remote", "get-url", name]).filter(|u| !u.is_empty())
}

/// How generated links name this worktree's repository: its remote's host form
/// (`github:o/n`, `bitbucket:w/n`, `bitbucket-dc:h/P/n`) when it has one Chorong recognises,
/// else `local:~/…` (or the absolute path when it is outside the home directory).
pub fn repo_identity(root: &Path) -> RepoRef {
    identity_remote(root)
        .and_then(|u| repo_ref_for_remote(&u))
        .unwrap_or_else(|| local_ref(root, home().as_deref()))
}

/// `local:~/…` for `root` under `home`, else `local:<absolute path>`.
pub fn local_ref(root: &Path, home: Option<&Path>) -> RepoRef {
    let rel = home.and_then(|h| root.strip_prefix(h).ok());
    let path = match rel {
        Some(r) if r.as_os_str().is_empty() => "~".to_string(),
        Some(r) => format!("~/{}", r.to_string_lossy().replace('\\', "/")),
        None => root.to_string_lossy().into_owned(),
    };
    RepoRef::Local { path }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .and_then(|h| h.canonicalize().ok().or(Some(h)))
}

/// One entry of `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Full commit id checked out there.
    pub head: Option<String>,
    /// Branch (short name), `None` when detached.
    pub branch: Option<String>,
}

/// Every worktree of the repository `root` belongs to, the main one first.
pub fn worktrees(root: &Path) -> Vec<Worktree> {
    git(root, &["worktree", "list", "--porcelain"])
        .map(|t| parse_worktrees(&t))
        .unwrap_or_default()
}

pub fn parse_worktrees(text: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    let mut cur: Option<Worktree> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            out.extend(cur.take());
            cur = Some(Worktree {
                path: PathBuf::from(p),
                head: None,
                branch: None,
            });
        } else if let Some(w) = cur.as_mut() {
            if let Some(h) = line.strip_prefix("HEAD ") {
                w.head = Some(h.to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                w.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            }
        }
    }
    out.extend(cur);
    out
}

/// A local worktree that might hold a linked commit.
pub type Candidate = Worktree;

/// Picks the worktree a link should open in: one already at the link's `rev` (a prefix of
/// its commit id), else one on the link's `branch`, else the first (most recently used).
pub fn choose_worktree<'a>(
    candidates: &'a [Candidate],
    rev: Option<&str>,
    branch: Option<&str>,
) -> Option<&'a Candidate> {
    let at_rev = |c: &&Candidate| {
        rev.zip(c.head.as_deref())
            .is_some_and(|(r, h)| r.len() >= 4 && h.starts_with(r))
    };
    let on_branch = |c: &&Candidate| branch.is_some() && c.branch.as_deref() == branch;
    candidates
        .iter()
        .find(at_rev)
        .or_else(|| candidates.iter().find(on_branch))
        .or_else(|| candidates.first())
}

/// Maps a link's repository to local worktrees. `known` are folders Chorong has opened (most
/// recent first); each one's sibling worktrees count too. Remote repositories match by remote
/// URL; `local:` ones by path. Returns candidates in preference order (see
/// [`choose_worktree`]), empty when none is known.
pub fn candidates_for(repo: &RepoRef, known: &[PathBuf]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let push = |out: &mut Vec<Candidate>, mut w: Worktree| {
        w.path = w.path.canonicalize().unwrap_or(w.path);
        if !out.iter().any(|c| c.path == w.path) {
            out.push(w);
        }
    };
    if let RepoRef::Local { .. } = repo {
        if let Some(p) = repo.local_path().filter(|p| p.is_dir()) {
            for w in worktrees(&p) {
                if w.path.canonicalize().ok() == p.canonicalize().ok() {
                    push(&mut out, w);
                }
            }
            if out.is_empty() {
                push(
                    &mut out,
                    Worktree {
                        path: p,
                        head: None,
                        branch: None,
                    },
                );
            }
        }
        return out;
    }
    for root in known.iter().filter(|r| r.is_dir()) {
        let same = identity_remote(root)
            .and_then(|u| repo_ref_for_remote(&u))
            .is_some_and(|r| &r == repo);
        if !same {
            continue;
        }
        // The folder itself first, then its sibling worktrees.
        let all = worktrees(root);
        let canonical = root.canonicalize().ok();
        let (mine, others): (Vec<_>, Vec<_>) = all
            .into_iter()
            .partition(|w| w.path.canonicalize().ok() == canonical);
        for w in mine.into_iter().chain(others) {
            push(&mut out, w);
        }
    }
    out
}

/// One changed range on the head side of a diff (one-based, inclusive lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// Head-side hunks of `git diff -U0 BASE..HEAD`, and the paths it deletes.
pub fn diff_hunks(
    root: &Path,
    base: &str,
    head: &str,
) -> Result<(Vec<DiffHunk>, Vec<String>), String> {
    let text = git(
        root,
        &[
            "diff",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=all",
            // Whatever the user's `diff.noprefix` or `diff.mnemonicPrefix` say.
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &format!("{base}..{head}"),
        ],
    )
    .ok_or_else(|| {
        format!("`git diff {base}..{head}` failed; are both commits in this repository?")
    })?;
    Ok(parse_diff(&text))
}

/// Parses the output of `git diff -U0` (see [`diff_hunks`]) into head-side hunks and the
/// paths it deletes.
///
/// Hunk bodies are skipped by their line counts, so a removed line `-- x` (shown as `--- x`)
/// or an added line `++ x` (`+++ x`) is not mistaken for a file header. File names are read
/// as git writes them: names with spaces end in a tab, and names with control characters,
/// quotes, backslashes or (by default) non-ASCII bytes are in `"…"` with C-style escapes.
pub fn parse_diff(text: &str) -> (Vec<DiffHunk>, Vec<String>) {
    let (mut hunks, mut removed) = (vec![], vec![]);
    let (mut path, mut old): (Option<String>, Option<String>) = (None, None);
    // Lines of the current hunk's body still to come: (removed, added).
    let (mut old_left, mut new_left) = (0usize, 0usize);
    for line in text.lines() {
        if old_left > 0 || new_left > 0 {
            let in_body = match line.as_bytes().first() {
                Some(b'-') if old_left > 0 => {
                    old_left -= 1;
                    true
                }
                Some(b'+') if new_left > 0 => {
                    new_left -= 1;
                    true
                }
                Some(b' ') if old_left > 0 && new_left > 0 => {
                    old_left -= 1;
                    new_left -= 1;
                    true
                }
                // "\ No newline at end of file".
                Some(b'\\') => true,
                _ => false,
            };
            if in_body {
                continue;
            }
            // The body ended early (truncated output): read this line as a header.
            (old_left, new_left) = (0, 0);
        }
        if line.starts_with("diff ") {
            (path, old) = (None, None);
        } else if let Some(p) = line.strip_prefix("--- ") {
            old = diff_path(p, "a/");
        } else if let Some(p) = line.strip_prefix("+++ ") {
            path = diff_path(p, "b/");
            if p == "/dev/null"
                && let Some(o) = old.take()
            {
                removed.push(o);
            }
        } else if let Some(h) = line.strip_prefix("@@ ")
            && let Some((minus, plus)) = h.split(' ').next().zip(h.split(' ').nth(1))
            && let Some(minus) = minus.strip_prefix('-')
            && let Some(plus) = plus.strip_prefix('+')
        {
            let range = |r: &str| {
                let (start, count) = r.split_once(',').map_or((r, "1"), |(a, b)| (a, b));
                Some((start.parse::<usize>().ok()?, count.parse::<usize>().ok()?))
            };
            let (Some((_, old_count)), Some((start, count))) = (range(minus), range(plus)) else {
                continue;
            };
            (old_left, new_left) = (old_count, count);
            let Some(p) = &path else { continue };
            // A pure deletion (count 0) sits between `start` and `start + 1`.
            let (s, e) = if count == 0 {
                (start.max(1), start + 1)
            } else {
                (start, start + count - 1)
            };
            hunks.push(DiffHunk {
                path: p.clone(),
                start_line: s,
                end_line: e,
            });
        }
    }
    (hunks, removed)
}

/// The path in a `---` / `+++` line (after the marker) without git's `a/` or `b/` prefix.
/// `None` for `/dev/null` and for anything without the prefix.
fn diff_path(raw: &str, prefix: &str) -> Option<String> {
    let raw = raw.strip_suffix('\t').unwrap_or(raw);
    let name = match raw.strip_prefix('"') {
        Some(q) => String::from_utf8_lossy(&unquote_c(q.strip_suffix('"')?)?).into_owned(),
        None => raw.to_string(),
    };
    name.strip_prefix(prefix)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

/// Undoes git's C-style quoting (`\n`, `\t`, `\"`, `\\`, `\303` octal bytes, …).
fn unquote_c(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let c = *b.get(i + 1)?;
        i += 2;
        out.push(match c {
            b'a' => 7,
            b'b' => 8,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 11,
            b'f' => 12,
            b'r' => b'\r',
            b'"' | b'\\' => c,
            b'0'..=b'3' => {
                let digits = b.get(i..i + 2)?;
                if !digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
                    return None;
                }
                i += 2;
                ((c - b'0') << 6) | ((digits[0] - b'0') << 3) | (digits[1] - b'0')
            }
            _ => return None,
        });
    }
    Some(out)
}

/// The bytes of `path` (repository-relative, `/`-separated) at `rev`, read from the object
/// store. The working tree and the checkout are not touched.
pub fn read_file_at(root: &Path, rev: &str, path: &str) -> Result<Vec<u8>, String> {
    let repo = gix::open(root).map_err(|e| format!("cannot open the repository: {e}"))?;
    let id = repo
        .rev_parse_single(rev)
        .map_err(|e| format!("`{rev}` is not in this repository: {e}"))?;
    let commit = id
        .object()
        .map_err(|e| e.to_string())?
        .peel_to_commit()
        .map_err(|e| format!("`{rev}` is not a commit: {e}"))?;
    let tree = commit.tree().map_err(|e| e.to_string())?;
    let entry = tree
        .lookup_entry_by_path(path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{path} does not exist at {}", short(&commit.id.to_string())))?;
    let object = entry.object().map_err(|e| e.to_string())?;
    if object.kind != gix::object::Kind::Blob {
        return Err(format!("{path} is not a file at {rev}"));
    }
    Ok(object.detach().data)
}

/// The command a reader can copy to move their own checkout to `branch` or `rev`. Chorong never
/// runs it. Both come from links, which are untrusted: each name is shell-quoted, and a name
/// git would read as an option (leading `-`) or an empty one gives no command at all.
pub fn switch_command(rev: &str, branch: Option<&str>) -> Option<String> {
    match branch {
        Some(b) => Some(format!("git switch {}", shell_word(b)?)),
        None => Some(format!("git switch --detach {}", shell_word(rev)?)),
    }
}

/// `s` as one POSIX shell word: unchanged when it holds only characters no shell treats
/// specially, otherwise in single quotes. `None` for an empty word or one starting with `-`.
fn shell_word(s: &str) -> Option<String> {
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let plain = s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"._/+@,=%-".contains(&b));
    Some(if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    })
}

#[cfg(test)]
mod tests;
