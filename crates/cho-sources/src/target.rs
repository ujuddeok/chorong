//! What to open: a branch, a pull request, a commit or a URL, parsed without touching the
//! network. Branch-or-commit questions that need the remote's refs are answered later by
//! [`crate::open`]; an input that could be either is an error naming both, never a guess.

use crate::{Error, Result};
use cho_core::{ChorongLink, RepoRef};

/// What a target names, before the remote's refs are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKind {
    /// The repository's default branch.
    Default,
    Branch(String),
    PullRequest(u64),
    /// A commit, named as such (a commit URL, `--rev`, a `chorong://` link's `rev`).
    Commit(String),
    /// Hexadecimal text that may be a commit or a branch of that name.
    BranchOrCommit(String),
    /// `REF/PATH` from a web URL (`tree/feat/x/src`): the branch is the longest prefix that
    /// names one, else the first part is a commit.
    RefPath(String),
}

/// A parsed target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The repository a URL named; `None` for plain text such as `#12` or `feat/x`.
    pub repo: Option<RepoRef>,
    pub kind: TargetKind,
    /// A file a URL pointed at (repository-relative), when it is not inside a `RefPath`.
    pub path: Option<String>,
    /// One-based line from a URL fragment.
    pub line: Option<usize>,
    /// A clone URL the input itself gives (a Data Center URL under a context path or port).
    pub clone_url: Option<String>,
    /// Before, from a `chorong://` link's `base` (a flag such as `--base` overrides it).
    pub base: Option<String>,
}

impl Target {
    fn new(kind: TargetKind) -> Self {
        Self {
            repo: None,
            kind,
            path: None,
            line: None,
            clone_url: None,
            base: None,
        }
    }
}

/// A repository named on its own: `github:acme/api`, `bitbucket:ws/n`,
/// `bitbucket-dc:host/P/n`, or any URL [`parse_target`] accepts (its branch or PR part is
/// ignored).
pub fn parse_repo(input: &str) -> Result<(RepoRef, Option<String>)> {
    let s = input.trim();
    if is_repo_name(s) {
        let r = RepoRef::parse(s).map_err(|e| Error::msg(e.to_string()))?;
        return Ok((r, None));
    }
    let t = parse_target(s)?;
    match t.repo {
        Some(r) => Ok((r, t.clone_url)),
        None => Err(Error::msg(format!(
            "`{s}` does not name a repository (expected github:OWNER/NAME, \
             bitbucket:WORKSPACE/NAME, bitbucket-dc:HOST/PROJECT/NAME or a repository URL)"
        ))),
    }
}

/// Parses what the user typed: empty (the default branch), `#123`, a branch, a commit, a
/// GitHub / Bitbucket Cloud / Bitbucket Data Center URL (repository, branch, file, commit or
/// pull request), a clone URL, or a `chorong://open` link.
pub fn parse_target(input: &str) -> Result<Target> {
    let t = parse_target_unchecked(input)?;
    // A repository built from URL parts must be one Chorong's own link form accepts.
    if let Some(r) = &t.repo
        && RepoRef::parse(&r.to_string()).ok().as_ref() != Some(r)
    {
        return Err(Error::msg(format!(
            "`{}` does not name a valid repository",
            input.trim()
        )));
    }
    Ok(t)
}

fn parse_target_unchecked(input: &str) -> Result<Target> {
    let s = input.trim();
    if s.is_empty() {
        return Ok(Target::new(TargetKind::Default));
    }
    if s.contains(char::is_control) {
        return Err(Error::msg(format!(
            "`{}` contains control characters",
            s.escape_debug()
        )));
    }
    if let Some(n) = s.strip_prefix('#') {
        return Ok(Target::new(TargetKind::PullRequest(parse_pr_number(n)?)));
    }
    if s.starts_with("chorong://") {
        return cho_link(s);
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return web_url(s);
    }
    if s.contains("://") || looks_scp(s) {
        cho_core::git::check_remote_url(s).map_err(Error::msg)?;
        let repo = cho_git::repo_ref_for_remote(s)
            .ok_or_else(|| Error::msg(format!("`{s}` is not a GitHub or Bitbucket clone URL")))?;
        let mut t = Target::new(TargetKind::Default);
        if matches!(repo, RepoRef::BitbucketDc { .. }) {
            t.clone_url = Some(s.to_string());
        }
        t.repo = Some(repo);
        return Ok(t);
    }
    if is_hex_commit(s) {
        return Ok(Target::new(TargetKind::BranchOrCommit(
            s.to_ascii_lowercase(),
        )));
    }
    check_branch_name(s)?;
    Ok(Target::new(TargetKind::Branch(s.to_string())))
}

pub fn parse_pr_number(n: &str) -> Result<u64> {
    match n.trim().parse::<u64>() {
        Ok(v) if v > 0 => Ok(v),
        _ => Err(Error::msg(format!("`{n}` is not a pull-request number"))),
    }
}

/// True when `s` is written as a repository name (`github:…`, `bitbucket:…`,
/// `bitbucket-dc:…`) rather than a URL.
pub fn is_repo_name(s: &str) -> bool {
    s.split_once(':')
        .is_some_and(|(kind, _)| matches!(kind, "github" | "bitbucket" | "bitbucket-dc"))
}

/// 7 to 40 hexadecimal digits.
pub fn is_hex_commit(s: &str) -> bool {
    (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The branch-name rules of `git check-ref-format --branch`, enough that a name is safe in
/// a refspec: no `..`, control characters, spaces or `~^:?*[\`, no leading `-` or `/`, no
/// trailing `/` or `.`, no part starting with `.` or ending in `.lock`, no `//` or `@{`.
pub fn check_branch_name(b: &str) -> Result<()> {
    let bad = b.is_empty()
        || b.starts_with('-')
        || b.starts_with('/')
        || b.ends_with('/')
        || b.ends_with('.')
        || b.contains("..")
        || b.contains("//")
        || b.contains("@{")
        || b == "@"
        || b.split('/')
            .any(|c| c.starts_with('.') || c.ends_with(".lock"))
        || b.chars()
            .any(|c| c.is_control() || c == ' ' || "~^:?*[\\".contains(c));
    if bad {
        Err(Error::msg(format!("`{b}` is not a valid branch name")))
    } else {
        Ok(())
    }
}

/// `git@host:path` (no scheme, no slash before the first colon).
fn looks_scp(s: &str) -> bool {
    match s.split_once(':') {
        Some((head, rest)) => {
            head.contains('@') && !head.contains('/') && !rest.is_empty() && !rest.starts_with("//")
        }
        None => false,
    }
}

fn cho_link(s: &str) -> Result<Target> {
    match ChorongLink::parse(s).map_err(|e| Error::msg(e.to_string()))? {
        ChorongLink::Open {
            repo,
            pr,
            rev,
            base,
            path,
            line,
            ..
        } => {
            if matches!(repo, RepoRef::Local { .. }) {
                return Err(Error::msg(format!(
                    "`{repo}` is a local folder; open it with `cho open PATH`"
                )));
            }
            let kind = match rev {
                // A pull-request link opens the pull request at its current head; its `rev`
                // is the head the link was made at, which the viewer compares.
                _ if pr.is_some() => TargetKind::PullRequest(pr.unwrap_or_default()),
                Some(r) if is_hex_commit(&r) => TargetKind::Commit(r.to_ascii_lowercase()),
                Some(r) => {
                    return Err(Error::msg(format!(
                        "the link's rev `{r}` is not a commit id"
                    )));
                }
                None => TargetKind::Default,
            };
            let mut t = Target::new(kind);
            t.repo = Some(repo);
            t.path = path;
            t.line = line;
            t.base = base;
            Ok(t)
        }
        ChorongLink::Investigation { .. } => Err(Error::msg(
            "an investigation link is not a repository; open it with `cho open LINK`",
        )),
    }
}

fn web_url(s: &str) -> Result<Target> {
    let url = url::Url::parse(s).map_err(|e| Error::msg(format!("`{s}`: {e}")))?;
    let host = url
        .host_str()
        .ok_or_else(|| Error::msg(format!("`{s}` has no host")))?
        .to_ascii_lowercase();
    let segs: Vec<String> = url
        .path_segments()
        .map(|p| p.filter(|x| !x.is_empty()).map(percent_decode).collect())
        .unwrap_or_default();
    if segs.iter().any(|s| s.contains(char::is_control)) {
        return Err(Error::msg(format!(
            "`{s}` names a path with control characters"
        )));
    }
    let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
    let fragment = url.fragment().unwrap_or("");
    let query = |k: &str| {
        url.query_pairs()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.into_owned())
    };
    let unsupported = |what: &str| {
        Error::unsupported(format!(
            "Chorong can't open this {what} URL: `{s}` (it opens repository, branch, file, commit \
             and pull-request URLs)"
        ))
    };
    match host.as_str() {
        "github.com" | "www.github.com" => {
            if segs.len() < 2 {
                return Err(unsupported("GitHub"));
            }
            let repo = RepoRef::GitHub {
                owner: segs[0].into(),
                name: strip_git(segs[1]).into(),
            };
            let rest = &segs[2..];
            let mut t = match rest {
                [] => Target::new(TargetKind::Default),
                ["tree" | "commits", r @ ..] if !r.is_empty() => {
                    Target::new(TargetKind::RefPath(r.join("/")))
                }
                ["blob", r @ ..] if r.len() >= 2 => {
                    let mut t = Target::new(TargetKind::RefPath(r.join("/")));
                    t.line = line_from(fragment, "L");
                    t
                }
                ["commit", sha, ..] if is_hex_commit(sha) => {
                    Target::new(TargetKind::Commit(sha.to_ascii_lowercase()))
                }
                ["pull", n, ..] => Target::new(TargetKind::PullRequest(parse_pr_number(n)?)),
                _ => return Err(unsupported("GitHub")),
            };
            t.repo = Some(repo);
            Ok(t)
        }
        "bitbucket.org" | "www.bitbucket.org" => {
            if segs.len() < 2 {
                return Err(unsupported("Bitbucket"));
            }
            let repo = RepoRef::Bitbucket {
                workspace: segs[0].into(),
                name: strip_git(segs[1]).into(),
            };
            let rest = &segs[2..];
            let mut t = match rest {
                [] => Target::new(TargetKind::Default),
                ["src", r @ ..] if !r.is_empty() => {
                    let mut t = Target::new(TargetKind::RefPath(r.join("/")));
                    t.line = line_from(fragment, "lines-");
                    t
                }
                ["branch", b @ ..] if !b.is_empty() => {
                    let b = b.join("/");
                    check_branch_name(&b)?;
                    Target::new(TargetKind::Branch(b))
                }
                ["commits", "branch", b @ ..] if !b.is_empty() => {
                    let b = b.join("/");
                    check_branch_name(&b)?;
                    Target::new(TargetKind::Branch(b))
                }
                ["commits", sha, ..] if is_hex_commit(sha) => {
                    Target::new(TargetKind::Commit(sha.to_ascii_lowercase()))
                }
                ["pull-requests", n, ..] => {
                    Target::new(TargetKind::PullRequest(parse_pr_number(n)?))
                }
                _ => return Err(unsupported("Bitbucket")),
            };
            t.repo = Some(repo);
            Ok(t)
        }
        _ => data_center(&url, &host, &segs, fragment, query("at")).unwrap_or_else(|| {
            Err(Error::msg(format!(
                "`{s}` is not a GitHub, Bitbucket Cloud or Bitbucket Data Center URL"
            )))
        }),
    }
}

/// Bitbucket Data Center: `[/ctx]/projects/P/repos/N/...` or `[/ctx]/scm/P/N.git`.
fn data_center(
    url: &url::Url,
    host: &str,
    segs: &[&str],
    fragment: &str,
    at: Option<String>,
) -> Option<Result<Target>> {
    let base = |ctx: &[&str]| {
        let mut b = format!("{}://{host}", url.scheme());
        if let Some(p) = url.port() {
            b.push_str(&format!(":{p}"));
        }
        for c in ctx {
            b.push('/');
            b.push_str(c);
        }
        b
    };
    if let Some(i) = segs.iter().position(|s| *s == "scm")
        && segs.len() == i + 3
    {
        let (project, name) = (segs[i + 1], strip_git(segs[i + 2]));
        let mut t = Target::new(TargetKind::Default);
        t.clone_url = Some(format!("{}/scm/{project}/{name}.git", base(&segs[..i])));
        t.repo = Some(RepoRef::BitbucketDc {
            host: host.into(),
            project: project.into(),
            name: name.into(),
        });
        return Some(Ok(t));
    }
    let i = segs
        .windows(4)
        .position(|w| w[0] == "projects" && w[2] == "repos")?;
    let (project, name) = (segs[i + 1], segs[i + 3]);
    let rest = &segs[i + 4..];
    let at_kind = |at: Option<String>| -> Result<TargetKind> {
        Ok(match at {
            None => TargetKind::Default,
            Some(a) => {
                if let Some(b) = a.strip_prefix("refs/heads/") {
                    check_branch_name(b)?;
                    TargetKind::Branch(b.into())
                } else if a.len() == 40 && is_hex_commit(&a) {
                    TargetKind::Commit(a.to_ascii_lowercase())
                } else if is_hex_commit(&a) {
                    TargetKind::BranchOrCommit(a.to_ascii_lowercase())
                } else if a.starts_with("refs/") {
                    return Err(Error::msg(format!(
                        "`{a}` is not a branch or commit (only refs/heads/ are opened)"
                    )));
                } else {
                    check_branch_name(&a)?;
                    TargetKind::Branch(a)
                }
            }
        })
    };
    let result = (|| {
        let mut t = match rest {
            [] => Target::new(at_kind(at)?),
            ["browse", p @ ..] => {
                let mut t = Target::new(at_kind(at)?);
                if !p.is_empty() {
                    t.path = Some(p.join("/"));
                    t.line = line_from(fragment, "");
                }
                t
            }
            ["commits", sha, ..] if is_hex_commit(sha) => {
                Target::new(TargetKind::Commit(sha.to_ascii_lowercase()))
            }
            ["pull-requests", n, ..] => Target::new(TargetKind::PullRequest(parse_pr_number(n)?)),
            _ => {
                return Err(Error::msg(format!(
                    "Chorong can't open this Bitbucket Data Center URL: `{url}`"
                )));
            }
        };
        t.clone_url = Some(format!("{}/scm/{project}/{name}.git", base(&segs[..i])));
        t.repo = Some(RepoRef::BitbucketDc {
            host: host.into(),
            project: project.into(),
            name: name.into(),
        });
        Ok(t)
    })();
    Some(result)
}

fn strip_git(s: &str) -> &str {
    s.strip_suffix(".git").unwrap_or(s)
}

/// The first line number after `prefix` in a fragment (`L10-L20` → 10, `lines-5:9` → 5).
fn line_from(fragment: &str, prefix: &str) -> Option<usize> {
    let rest = fragment.strip_prefix(prefix)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|n| *n >= 1)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How a `RefPath` resolves against the remote's branch names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefPathMatch {
    Branch {
        branch: String,
        path: Option<String>,
    },
    Commit {
        commit: String,
        path: Option<String>,
    },
}

/// Splits `REF/PATH` using the remote's branches: the longest prefix that is a branch (git
/// forbids both `a` and `a/b`, so at most one matches), else a leading commit id. A leading
/// commit id that is also a branch name is an error naming both.
pub fn resolve_ref_path(refpath: &str, branches: &[String]) -> Result<RefPathMatch> {
    let parts: Vec<&str> = refpath.split('/').collect();
    let path_after = |n: usize| (n < parts.len()).then(|| parts[n..].join("/"));
    for n in (1..=parts.len()).rev() {
        let candidate = parts[..n].join("/");
        if branches.contains(&candidate) {
            if n == 1 && is_hex_commit(&candidate) {
                return Err(ambiguous(&candidate));
            }
            return Ok(RefPathMatch::Branch {
                branch: candidate,
                path: path_after(n),
            });
        }
    }
    if is_hex_commit(parts[0]) {
        return Ok(RefPathMatch::Commit {
            commit: parts[0].to_ascii_lowercase(),
            path: path_after(1),
        });
    }
    Err(Error::msg(format!(
        "`{refpath}` does not start with a branch of this repository or a commit id"
    )))
}

pub(crate) fn ambiguous(name: &str) -> Error {
    Error::new(
        crate::ErrorKind::Ambiguous {
            name: name.to_string(),
        },
        format!(
            "`{name}` is both a branch and a commit id; say which: --branch {name} or --rev {name}"
        ),
    )
}

#[cfg(test)]
mod tests;
