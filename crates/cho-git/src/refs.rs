//! The refs a repository has, for "Compare against…" and any other picker of revisions.
//! Read through `gix` only; works on bare repositories.

use std::path::Path;

/// What kind of ref an entry is. Sorts in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefKind {
    Branch,
    RemoteBranch,
    Tag,
}

/// One branch, remote-tracking branch or tag.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefEntry {
    /// Short name: `main`, `origin/main`, `v1.0`.
    pub short: String,
    pub kind: RefKind,
    /// The commit it points at (full hex id); annotated tags are peeled to their commit.
    pub target: String,
}

/// Local branches, remote-tracking branches and tags, sorted by (kind, short name). Remote
/// `HEAD` aliases and refs that do not lead to a commit are left out.
pub fn list_refs(root: &Path) -> Result<Vec<RefEntry>, String> {
    let repo = gix::discover(root).map_err(|e| format!("cannot open the repository: {e}"))?;
    let platform = repo.references().map_err(|e| e.to_string())?;
    let all = platform.all().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in all {
        let mut r = r.map_err(|e| e.to_string())?;
        let full = r.name().as_bstr().to_string();
        let kind = if full.starts_with("refs/heads/") {
            RefKind::Branch
        } else if full.starts_with("refs/remotes/") && !full.ends_with("/HEAD") {
            RefKind::RemoteBranch
        } else if full.starts_with("refs/tags/") {
            RefKind::Tag
        } else {
            continue;
        };
        let short = r.name().shorten().to_string();
        let Ok(id) = r.peel_to_id() else { continue };
        let Ok(commit) = id.object().and_then(|o| o.peel_to_commit()) else {
            continue;
        };
        out.push(RefEntry {
            short,
            kind,
            target: commit.id.to_string(),
        });
    }
    out.sort_by(|a, b| (a.kind, &a.short).cmp(&(b.kind, &b.short)));
    out.dedup_by(|a, b| a.kind == b.kind && a.short == b.short);
    Ok(out)
}

/// Which refs a picker should offer first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreferredRefs {
    /// The default branch's name without a remote (`main`): the branch the remote's HEAD
    /// points at (`origin` first), else `main`, else `master`, whichever the repository has.
    pub default_branch: Option<String>,
    /// The short name of the current branch's upstream (`origin/feature`), if it has one.
    pub upstream: Option<String>,
}

/// The default branch and the current branch's upstream. Read through `gix` only.
pub fn preferred_refs(root: &Path) -> PreferredRefs {
    let Ok(repo) = gix::discover(root) else {
        return PreferredRefs::default();
    };
    let mut remote_heads: Vec<(String, String)> = Vec::new();
    if let Ok(platform) = repo.references()
        && let Ok(all) = platform.all()
    {
        for r in all.flatten() {
            let full = r.name().as_bstr().to_string();
            let Some(rest) = full.strip_prefix("refs/remotes/") else {
                continue;
            };
            let Some(remote) = rest.strip_suffix("/HEAD") else {
                continue;
            };
            if let Some(target) = r.target().try_name() {
                let target = target.as_bstr().to_string();
                if let Some(branch) = target.strip_prefix(&format!("refs/remotes/{remote}/")) {
                    remote_heads.push((remote.to_string(), branch.to_string()));
                }
            }
        }
    }
    remote_heads.sort_by_key(|(remote, _)| (remote != "origin", remote.clone()));
    let has = |full: String| repo.find_reference(&full).is_ok();
    let default_branch = remote_heads
        .first()
        .map(|(_, branch)| branch.clone())
        .or_else(|| {
            ["main", "master"]
                .into_iter()
                .find(|b| has(format!("refs/heads/{b}")))
                .map(str::to_string)
        });
    let upstream = repo
        .head_ref()
        .ok()
        .flatten()
        .and_then(|h| h.remote_tracking_ref_name(gix::remote::Direction::Fetch))
        .and_then(Result::ok)
        .filter(|n| has(n.as_bstr().to_string()))
        .map(|n| n.shorten().to_string());
    PreferredRefs {
        default_branch,
        upstream,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cho_core::test_repo::git as sh;

    fn names(v: &[RefEntry]) -> Vec<(RefKind, &str)> {
        v.iter().map(|e| (e.kind, e.short.as_str())).collect()
    }

    #[test]
    fn lists_branches_remote_branches_and_peeled_tags_sorted_by_kind_then_name() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        sh(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("a.txt"), "1\n").unwrap();
        sh(d, &["add", "."]);
        sh(d, &["commit", "-q", "-m", "one"]);
        let head = crate::rev_parse(d, "HEAD").unwrap();
        sh(d, &["branch", "feature/x"]);
        sh(d, &["tag", "v1.0"]);
        sh(d, &["tag", "-a", "-m", "annotated", "v2.0"]);
        sh(d, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        sh(
            d,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        let refs = list_refs(d).unwrap();
        assert_eq!(
            names(&refs),
            vec![
                (RefKind::Branch, "feature/x"),
                (RefKind::Branch, "main"),
                (RefKind::RemoteBranch, "origin/main"),
                (RefKind::Tag, "v1.0"),
                (RefKind::Tag, "v2.0"),
            ]
        );
        assert!(refs.iter().all(|e| e.target == head), "tags are peeled");
        assert!(list_refs(&d.join("missing")).is_err());
        // From a subfolder of the repository (the viewer opened on one, or on a single file).
        std::fs::create_dir(d.join("sub")).unwrap();
        assert_eq!(list_refs(&d.join("sub")).unwrap(), refs);
        assert_eq!(
            preferred_refs(&d.join("sub")),
            preferred_refs(d),
            "preferred refs are found from a subfolder too"
        );
    }

    #[test]
    fn works_on_a_bare_clone() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        sh(&src, &["init", "-q", "-b", "main"]);
        std::fs::write(src.join("a.txt"), "1\n").unwrap();
        sh(&src, &["add", "."]);
        sh(&src, &["commit", "-q", "-m", "one"]);
        sh(&src, &["tag", "v1"]);
        let bare = dir.path().join("bare.git");
        sh(
            dir.path(),
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let refs = list_refs(&bare).unwrap();
        assert_eq!(
            names(&refs),
            vec![(RefKind::Branch, "main"), (RefKind::Tag, "v1")]
        );
    }

    #[test]
    fn finds_the_default_branch_and_the_upstream() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        sh(&src, &["init", "-q", "-b", "trunk"]);
        std::fs::write(src.join("a.txt"), "1\n").unwrap();
        sh(&src, &["add", "."]);
        sh(&src, &["commit", "-q", "-m", "one"]);
        // No remote: `main`/`master` only when present; `trunk` is not guessed.
        assert_eq!(preferred_refs(&src).default_branch, None);
        sh(&src, &["branch", "master"]);
        assert_eq!(
            preferred_refs(&src).default_branch.as_deref(),
            Some("master")
        );
        sh(&src, &["branch", "main"]);
        assert_eq!(preferred_refs(&src).default_branch.as_deref(), Some("main"));
        assert_eq!(preferred_refs(&src).upstream, None);
        // A clone: the remote's HEAD names the default, the branch tracks its upstream.
        let clone = dir.path().join("clone");
        sh(
            dir.path(),
            &[
                "clone",
                "-q",
                src.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        sh(
            &clone,
            &["switch", "-q", "-c", "topic", "--track", "origin/master"],
        );
        let p = preferred_refs(&clone);
        assert_eq!(p.default_branch.as_deref(), Some("trunk"));
        assert_eq!(p.upstream.as_deref(), Some("origin/master"));
    }
}
