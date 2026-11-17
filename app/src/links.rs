//! Opening `chorong://` links: which local worktree a link's repository maps to, and making
//! links to the place being read.
//!
//! A link names a repository by its remote (`github:acme/api`) or a `local:` path. Remote
//! links resolve to a worktree Chorong has opened before, matched by remote URL, preferring one
//! already at the link's commit or branch; with none, the viewer offers the web URL instead.
//! Nothing is fetched or checked out. See `docs/public/url-scheme.md`.

use cho_core::{ChorongLink, LinkSide, RepoRef, web_url};
use std::path::{Path, PathBuf};

/// What a link said about where it was made, kept for the commit-mismatch banner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkInfo {
    pub url: String,
    pub repo: String,
    pub rev: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub path: Option<String>,
    pub symbol: Option<String>,
    /// One-based.
    pub line: Option<usize>,
    /// The link came from a graph page (`from=graph`), so "Rebuild graph" applies.
    pub from_graph: bool,
    /// Pull request number (`pr`, provisional).
    pub pr: Option<u64>,
    /// Before, a full commit id (`base`, provisional).
    pub base: Option<String>,
    /// The side the link was made on (`side`, provisional).
    pub side: Option<LinkSide>,
    /// Chorong fetched the pull request for this link just now (so a missing `rev` is gone from
    /// the host, not merely unfetched).
    pub fetched: bool,
}

impl LinkInfo {
    /// The link asks for a Before/After view (a `pr`, or a `base`).
    pub fn wants_comparison(&self) -> bool {
        self.pr.is_some() || self.base.is_some()
    }
}

/// Where a link leads on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// A local worktree of the repository.
    Local { root: PathBuf, info: LinkInfo },
    /// No worktree of this repository is known here; `web` is the page on its host, if any.
    Elsewhere { info: LinkInfo, web: Option<String> },
    /// A pull-request link and no cache worktree for that pull request: the reader is asked
    /// before Chorong fetches it.
    AskFetch {
        info: LinkInfo,
        repo: RepoRef,
        number: u64,
        /// A local worktree of the repository at the link's `rev`: offered as a second
        /// choice that fetches nothing.
        local: Option<PathBuf>,
    },
    /// `chorong://investigation?src=…`: an investigation file (absolute path) or https URL.
    /// Opening it reads only that file; nothing it names is fetched or run.
    Investigation(crate::investigation::Source),
}

/// The worktree among `candidates` that is at `rev` (a prefix of its commit id, at least
/// four digits). Unlike `choose_worktree` there is no fallback to another worktree: the
/// question offers a local folder only when it really shows the linked commit.
pub fn local_at_rev(candidates: &[cho_git::Candidate], rev: Option<&str>) -> Option<PathBuf> {
    let rev = rev.filter(|r| r.len() >= 4)?;
    candidates
        .iter()
        .find(|c| c.head.as_deref().is_some_and(|h| h.starts_with(rev)))
        .map(|c| c.path.clone())
}

/// The link for the local choice: the same place and comparison in `folder`, with no `pr`.
pub fn local_link(info: &LinkInfo, folder: &Path) -> Option<String> {
    let link = ChorongLink::Open {
        repo: RepoRef::Local {
            path: folder.display().to_string(),
        },
        pr: None,
        rev: info.rev.clone(),
        base: info.base.clone(),
        side: info.side,
        path: info.path.clone(),
        symbol: info.symbol.clone(),
        line: info.line,
        branch: None,
        dirty: false,
    };
    let url = link.to_url();
    ChorongLink::parse(&url).ok().map(|_| url)
}

/// `path` with the home folder shown as `~`.
pub fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    match home
        .as_deref()
        .map(Path::new)
        .and_then(|h| path.strip_prefix(h).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Resolves a `chorong://open` link against the worktrees Chorong knows (`known`, most recent
/// first). A `chorong://investigation` link resolves to its source.
pub fn resolve(url: &str, known: &[PathBuf]) -> Result<Resolved, String> {
    resolve_with(url, known, &crate::remote::adapter::pr_worktree)
}

/// [`resolve`] with the lookup of a pull request's cache worktree given (tests pass their own).
///
/// The open decision, in order: a `pr` link on a remote repository goes to the cache worktree
/// of that pull request if there is one ([`Resolved::Local`]) and otherwise asks first
/// ([`Resolved::AskFetch`]); every other link resolves as before (a local worktree at `rev`
/// preferred), and its `base`, if any, starts a Before/After view in that worktree.
pub fn resolve_with(
    url: &str,
    known: &[PathBuf],
    pr_worktree: &dyn Fn(&RepoRef, u64) -> Option<PathBuf>,
) -> Result<Resolved, String> {
    let link = ChorongLink::parse(url).map_err(|e| e.to_string())?;
    if let ChorongLink::Investigation { src } = &link {
        return crate::investigation::Source::from_link_src(src)
            .map(Resolved::Investigation)
            .map_err(|e| e.to_string());
    }
    let ChorongLink::Open {
        repo,
        pr,
        base,
        side,
        rev,
        path,
        symbol,
        line,
        branch,
        dirty,
    } = link
    else {
        unreachable!("investigation links return above");
    };
    let info = LinkInfo {
        url: url.to_string(),
        repo: repo.to_string(),
        rev: rev.clone(),
        branch: branch.clone(),
        dirty,
        path: path.clone(),
        symbol,
        line,
        from_graph: url_param(url, "from").as_deref() == Some("graph"),
        // A pull request means nothing for a local folder.
        pr: pr.filter(|_| !matches!(repo, RepoRef::Local { .. })),
        base,
        side,
        fetched: false,
    };
    if let Some(number) = info.pr {
        return Ok(match pr_worktree(&repo, number) {
            Some(root) => Resolved::Local { root, info },
            None => {
                // Not the cache's own worktrees: a folder of the reader's.
                let candidates: Vec<_> = cho_git::candidates_for(&repo, known)
                    .into_iter()
                    .filter(|c| crate::remote::adapter::worktree_project(&c.path).is_none())
                    .collect();
                let local = local_at_rev(&candidates, info.rev.as_deref());
                Resolved::AskFetch {
                    info,
                    repo,
                    number,
                    local,
                }
            }
        });
    }
    let candidates = cho_git::candidates_for(&repo, known);
    if let Some(c) = cho_git::choose_worktree(&candidates, rev.as_deref(), branch.as_deref()) {
        return Ok(Resolved::Local {
            root: c.path.clone(),
            info,
        });
    }
    if let RepoRef::Local { .. } = repo {
        return Err(format!("{repo}: no such folder on this machine"));
    }
    let at = rev.or(branch).unwrap_or_else(|| "HEAD".into());
    let web = web_url(&repo, &at, path.as_deref().unwrap_or(""), line);
    Ok(Resolved::Elsewhere { info, web })
}

/// The raw value of query parameter `key` (for parameters `ChorongLink` doesn't model).
fn url_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// What a window adds to the links it copies: the pull request, and the comparison and side
/// it is showing. `None` for a window that is neither (links stay as they were).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewParams {
    /// The repository as the cache names it (a cache worktree's own remote may be a path).
    pub repo: Option<RepoRef>,
    pub pr: Option<u64>,
    /// The head (After) as a full commit id.
    pub head: Option<String>,
    /// The comparison's Before, as a full commit id.
    pub base: Option<String>,
    pub side: Option<LinkSide>,
}

impl ViewParams {
    /// From what the window shows: the pull request (if it is a pull-request view), its head,
    /// the comparison's Before when comparing, and the active pane's side. `None` when the
    /// window is neither a pull-request view nor comparing.
    pub fn of(
        repo: Option<RepoRef>,
        pr: Option<u64>,
        head: Option<&str>,
        base: Option<&str>,
        side: LinkSide,
    ) -> Option<Self> {
        if pr.is_none() && base.is_none() {
            return None;
        }
        Some(Self {
            repo,
            pr,
            head: head.map(str::to_string),
            base: base.map(str::to_string),
            side: base.map(|_| side),
        })
    }
}

/// The link to `file` (an absolute path in the worktree `root`) at `symbol` (or `line`,
/// one-based): the repository by its remote (or `local:~/…`), the commit, and, unless
/// `share`, the branch and dirty state. The path is relative to the repository's top level,
/// which may be above the folder the viewer opened. `view` adds `pr`, `base` and `side`
/// (provisional) for a pull-request view or a comparison.
pub fn link_to(
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<usize>,
    share: bool,
    view: Option<&ViewParams>,
) -> Result<String, String> {
    let top = cho_git::toplevel(root);
    let top = top.canonicalize().unwrap_or(top);
    let file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = file
        .strip_prefix(&top)
        .map_err(|_| format!("{} is outside the repository", file.display()))?;
    let git = cho_git::info(&top);
    let repo = view
        .and_then(|v| v.repo.clone())
        .unwrap_or_else(|| cho_git::repo_identity(&top));
    if share {
        repo.check_shareable()?;
    }
    let rev = match view {
        // A shared view is reproducible: the full head, not an abbreviation.
        Some(v) => v.head.clone().or(git.full_commit.clone()),
        None => git.commit,
    };
    let link = ChorongLink::Open {
        repo,
        pr: view.and_then(|v| v.pr),
        rev,
        base: view.and_then(|v| v.base.clone()),
        side: view.and_then(|v| v.side),
        path: Some(rel.to_string_lossy().replace('\\', "/")),
        symbol: symbol.map(str::to_string),
        line: if symbol.is_none() { line } else { None },
        branch: if share { None } else { git.branch },
        dirty: !share && git.dirty,
    };
    Ok(link.to_url())
}

/// Which Before a link's comparison starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeforeChoice {
    /// The link's `base`, which this repository has (full commit id).
    Link(String),
    /// The merge base with the target instead: the link had no `base`, or the one it named is
    /// not available (`missing`, to say so).
    MergeBase { missing: bool },
}

/// The Before a link's comparison starts with. `found` is the link's `base` as a full commit
/// id when this repository has it. The notice for a missing one is [`MISSING_BASE`].
pub fn choose_before(base: Option<&str>, found: Option<String>) -> BeforeChoice {
    match (base, found) {
        (Some(_), Some(full)) => BeforeChoice::Link(full),
        (Some(_), None) => BeforeChoice::MergeBase { missing: true },
        (None, _) => BeforeChoice::MergeBase { missing: false },
    }
}

/// Said when a link's `base` is not in the cache even after fetching.
pub const MISSING_BASE: &str = "That Before commit isn't available.";

/// The banner text of a pull-request link whose `rev` is not in the cache. `fetched`: Chorong
/// has just fetched the pull request, so the version is gone from the host; otherwise it may
/// only be unfetched.
pub fn gone_text(rev: &str, here: &str, fetched: bool) -> String {
    if fetched {
        format!(
            "That version of the pull request is gone: {rev} is not on the host any more (a \
             force-push replaced it). This window shows the current head {here}."
        )
    } else {
        format!(
            "That version of the pull request isn't in Chorong's cache: {rev} was replaced by a \
             force-push, or not fetched yet. This window shows {here}."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_links_with_no_known_worktree_offer_the_web_page() {
        let url = "chorong://open?repo=github:acme/api&rev=3f9c1e2&path=src/lib.rs&symbol=Queue::push&line=12";
        let r = resolve(url, &[]).unwrap();
        let Resolved::Elsewhere { info, web } = &r else {
            panic!("expected Elsewhere, got {r:?}");
        };
        assert_eq!(info.rev.as_deref(), Some("3f9c1e2"));
        assert_eq!(
            web.as_deref(),
            Some("https://github.com/acme/api/blob/3f9c1e2/src/lib.rs#L12")
        );
        assert!(!info.from_graph);
    }

    #[test]
    fn local_links_open_their_folder_and_graph_links_are_recognised() {
        let dir = std::env::temp_dir();
        let url = format!(
            "chorong://open?repo=local:{}&path=a.rs&from=graph",
            dir.display()
        );
        match resolve(&url, &[]).unwrap() {
            Resolved::Local { root, info } => {
                assert_eq!(root.canonicalize().ok(), dir.canonicalize().ok());
                assert!(info.from_graph);
            }
            other => panic!("{other:?}"),
        }
        assert!(resolve("chorong://open?repo=local:/no/such/chorong/dir", &[]).is_err());
        assert!(matches!(
            resolve("chorong://investigation?src=/x.json", &[]),
            Ok(Resolved::Investigation(crate::investigation::Source::Path(
                _
            )))
        ));
        assert!(matches!(
            resolve(
                "chorong://investigation?src=https://example.com/a.json",
                &[]
            ),
            Ok(Resolved::Investigation(crate::investigation::Source::Url(
                _
            )))
        ));
        for bad in ["x.json", "http://example.com/a.json", "file:///etc/passwd"] {
            let url = cho_core::ChorongLink::Investigation { src: bad.into() }.to_url();
            assert!(resolve(&url, &[]).is_err(), "{bad}");
        }
    }

    use cho_core::test_repo::git;

    #[test]
    fn copied_links_name_the_remote_and_sharing_drops_branch_and_dirty() {
        let dir_guard = tempfile::tempdir().unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(dir.join("sub/src")).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("sub/src/a.rs"), "fn a() {}\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "one"]);
        git(
            &dir,
            &["remote", "add", "origin", "git@github.com:acme/demo.git"],
        );
        std::fs::write(dir.join("sub/src/a.rs"), "fn a() { }\n").unwrap();
        let rev = cho_git::info(&dir).commit.unwrap();

        // Opened at a subfolder: the path is still relative to the repository's top level.
        let file = dir.join("sub/src/a.rs");
        let full = link_to(&dir.join("sub"), &file, Some("a"), Some(1), false, None).unwrap();
        assert_eq!(
            full,
            format!(
                "chorong://open?repo=github:acme/demo&rev={rev}&path=sub/src/a.rs&symbol=a&branch=main&dirty=1"
            )
        );
        let shared = link_to(&dir.join("sub"), &file, None, Some(1), true, None).unwrap();
        assert_eq!(
            shared,
            format!("chorong://open?repo=github:acme/demo&rev={rev}&path=sub/src/a.rs&line=1")
        );
        let tmp = dir.to_string_lossy().into_owned();
        assert!(
            !full.contains(&tmp) && !shared.contains(&tmp),
            "no local path"
        );
    }

    #[test]
    fn sharing_refuses_a_repository_outside_home_with_no_remote() {
        let dir_guard = tempfile::tempdir().unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "one"]);
        let file = dir.join("a.rs");
        // The temp folder is outside the home folder, so the repository is `local:/…`. If it
        // is not, this test would check nothing: say so instead of passing.
        assert!(
            cho_git::repo_identity(&dir).is_absolute_local(),
            "{} is under the home folder; run with TMPDIR outside it",
            dir.display()
        );
        let err = link_to(&dir, &file, None, Some(1), true, None).unwrap_err();
        assert!(
            err.contains("Add a remote") && err.contains("--repo"),
            "{err}"
        );
        // Own links are unchanged.
        let own = link_to(&dir, &file, None, Some(1), false, None).unwrap();
        assert!(own.starts_with("chorong://open?repo=local:/"), "{own}");
    }

    const BASE: &str = "0123456789abcdef0123456789abcdef01234567";
    const HEAD: &str = "89abcdef0123456789abcdef0123456789abcdef";
    const PR_LINK: &str = "chorong://open?repo=github:acme/api&pr=12&rev=89abcdef0123456789abcdef0123456789abcdef&base=0123456789abcdef0123456789abcdef01234567&side=before&path=src/lib.rs&symbol=Queue::push";

    #[test]
    fn a_pull_request_link_goes_to_its_cache_worktree_when_there_is_one() {
        let wt = PathBuf::from("/cache/github/acme/api/worktrees/pr-12");
        let found = |_: &RepoRef, n: u64| (n == 12).then(|| wt.clone());
        let Resolved::Local { root, info } = resolve_with(PR_LINK, &[], &found).unwrap() else {
            panic!("expected the PR worktree");
        };
        assert_eq!(root, wt);
        assert_eq!((info.pr, info.base.as_deref()), (Some(12), Some(BASE)));
        assert_eq!(info.side, Some(LinkSide::Before));
        assert_eq!(info.rev.as_deref(), Some(HEAD));
        assert!(info.wants_comparison());
    }

    #[test]
    fn a_pull_request_link_with_nothing_cached_asks_before_fetching() {
        let none = |_: &RepoRef, _: u64| None;
        // Even with a worktree of the repository known: the pull request is not fetched
        // without being asked.
        let known = [std::env::temp_dir()];
        let r = resolve_with(PR_LINK, &known, &none).unwrap();
        let Resolved::AskFetch {
            info, repo, number, ..
        } = r
        else {
            panic!("expected a question, got {r:?}");
        };
        assert_eq!(number, 12);
        assert_eq!(repo.to_string(), "github:acme/api");
        assert_eq!(info.pr, Some(12));
    }

    #[test]
    fn rev_and_base_without_a_pull_request_resolve_as_before() {
        let url = format!("chorong://open?repo=github:acme/api&rev={HEAD}&base={BASE}&path=a.rs");
        let none = |_: &RepoRef, _: u64| panic!("no pull request, no cache lookup");
        // No worktree known: the web page, as for any link.
        let Resolved::Elsewhere { info, .. } = resolve_with(&url, &[], &none).unwrap() else {
            panic!()
        };
        assert_eq!((info.pr, info.base.as_deref()), (None, Some(BASE)));
        assert!(info.wants_comparison());
        // A local repository ignores `pr`.
        let local = format!(
            "chorong://open?repo=local:{}&pr=3&path=a.rs",
            std::env::temp_dir().display()
        );
        let Resolved::Local { info, .. } = resolve_with(&local, &[], &none).unwrap() else {
            panic!()
        };
        assert_eq!(info.pr, None);
    }

    #[test]
    fn links_without_the_new_parameters_resolve_as_today() {
        let none = |_: &RepoRef, _: u64| panic!("no cache lookup");
        let r = resolve_with(
            "chorong://open?repo=github:acme/api&rev=3f9c1e2",
            &[],
            &none,
        )
        .unwrap();
        let Resolved::Elsewhere { info, .. } = r else {
            panic!()
        };
        assert!(!info.wants_comparison());
        assert_eq!((info.pr, info.base, info.side), (None, None, None));
    }

    #[test]
    fn the_before_is_the_links_base_or_the_merge_base_with_a_notice_when_missing() {
        assert_eq!(
            choose_before(Some(BASE), Some(BASE.into())),
            BeforeChoice::Link(BASE.into())
        );
        assert_eq!(
            choose_before(Some(BASE), None),
            BeforeChoice::MergeBase { missing: true }
        );
        assert_eq!(
            choose_before(None, None),
            BeforeChoice::MergeBase { missing: false }
        );
        assert!(MISSING_BASE.contains("Before commit isn't available"));
    }

    #[test]
    fn a_gone_version_says_so() {
        let fetched = gone_text("89abcde", "1234567", true);
        assert!(
            fetched.starts_with("That version of the pull request is gone"),
            "{fetched}"
        );
        assert!(fetched.contains("1234567"));
        let cached = gone_text("89abcde", "1234567", false);
        assert!(cached.contains("isn't in Chorong's cache"), "{cached}");
    }

    #[test]
    fn copied_links_carry_the_pull_request_comparison_and_side() {
        let dir_guard = tempfile::tempdir().unwrap();
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "one"]);
        git(
            &dir,
            &["remote", "add", "origin", "git@github.com:acme/demo.git"],
        );
        let rev = cho_git::info(&dir).full_commit.unwrap();
        let short = cho_git::short(&rev);
        let file = dir.join("src/a.rs");

        // Plain: as today (abbreviated commit, no new parameters).
        let plain = link_to(&dir, &file, Some("a"), Some(1), true, None).unwrap();
        assert_eq!(
            plain,
            format!("chorong://open?repo=github:acme/demo&rev={short}&path=src/a.rs&symbol=a")
        );
        // Not a pull request and not comparing: still as today.
        assert_eq!(
            ViewParams::of(None, None, Some(&rev), None, LinkSide::After),
            None
        );

        // A comparison with no pull request: rev, base and side.
        let cmp = ViewParams::of(None, None, Some(&rev), Some(BASE), LinkSide::Before);
        let link = link_to(&dir, &file, Some("a"), Some(1), true, cmp.as_ref()).unwrap();
        assert_eq!(
            link,
            format!(
                "chorong://open?repo=github:acme/demo&rev={rev}&base={BASE}&side=before&path=src/a.rs&symbol=a"
            )
        );

        // A pull-request view: the cache's own name for the repository, and the pull request.
        let repo = RepoRef::parse("github:acme/api").unwrap();
        let pr = ViewParams::of(
            Some(repo.clone()),
            Some(12),
            Some(&rev),
            Some(BASE),
            LinkSide::After,
        );
        let link = link_to(&dir, &file, None, Some(1), true, pr.as_ref()).unwrap();
        assert_eq!(
            link,
            format!(
                "chorong://open?repo=github:acme/api&pr=12&rev={rev}&base={BASE}&side=after&path=src/a.rs&line=1"
            )
        );
        // The link parses back to the same view, and a pull request that is not comparing
        // carries no base or side.
        let ChorongLink::Open { pr, base, side, .. } = ChorongLink::parse(&link).unwrap() else {
            panic!()
        };
        assert_eq!(
            (pr, base.as_deref(), side),
            (Some(12), Some(BASE), Some(LinkSide::After))
        );
        let bare = ViewParams::of(Some(repo), Some(12), Some(&rev), None, LinkSide::Before);
        let link = link_to(&dir, &file, None, Some(1), true, bare.as_ref()).unwrap();
        assert_eq!(
            link,
            format!("chorong://open?repo=github:acme/api&pr=12&rev={rev}&path=src/a.rs&line=1")
        );
    }

    #[test]
    fn a_local_worktree_at_rev_is_offered_only_when_there_is_one() {
        let c = |p: &str, head: Option<&str>| cho_git::Candidate {
            path: PathBuf::from(p),
            head: head.map(str::to_string),
            branch: None,
        };
        let cands = [c("/w/a", Some("1111111aaa")), c("/w/b", Some(HEAD))];
        assert_eq!(
            local_at_rev(&cands, Some(&HEAD[..7])),
            Some(PathBuf::from("/w/b"))
        );
        assert_eq!(local_at_rev(&cands, Some("2222222")), None);
        assert_eq!(
            local_at_rev(&cands, Some("abc")),
            None,
            "too short to match"
        );
        assert_eq!(local_at_rev(&cands, None), None);
        assert_eq!(local_at_rev(&[], Some("1111111")), None);
        assert_eq!(local_at_rev(&[c("/w/c", None)], Some("1111111")), None);
    }

    #[test]
    fn the_local_choice_opens_the_same_place_without_the_pull_request() {
        let none = |_: &RepoRef, _: u64| None;
        let Resolved::AskFetch { info, .. } = resolve_with(PR_LINK, &[], &none).unwrap() else {
            panic!()
        };
        let url = local_link(&info, Path::new("/home/me/api")).unwrap();
        assert_eq!(
            url,
            format!(
                "chorong://open?repo=local:/home/me/api&rev={HEAD}&base={BASE}&side=before&path=src/lib.rs&symbol=Queue::push"
            )
        );
        let ChorongLink::Open { pr, base, .. } = ChorongLink::parse(&url).unwrap() else {
            panic!()
        };
        assert_eq!((pr, base.as_deref()), (None, Some(BASE)));
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let p = PathBuf::from(&home).join("src/api");
            assert_eq!(tilde(&p), "~/src/api");
        }
        assert_eq!(tilde(Path::new("/srv/api")), "/srv/api");
    }
}
