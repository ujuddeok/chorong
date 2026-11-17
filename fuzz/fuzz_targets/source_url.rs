#![no_main]
//! What the reader types or a link carries before Chorong runs git with it: targets
//! (branches, PRs, commits, web and clone URLs), repository names, branch names, remote
//! URLs and hosts. Nothing accepted may reach git as an option, a remote helper or an
//! invalid ref.
use libfuzzer_sys::fuzz_target;
use cho_core::RepoRef;
use cho_core::git::check_remote_url;
use cho_sources::target::{RefPathMatch, check_branch_name, is_hex_commit, resolve_ref_path};
use cho_sources::{TargetKind, parse_repo, parse_target, url_host};

/// `git check-ref-format --branch`, written from git's rules rather than Chorong's.
fn git_accepts_branch(b: &str) -> bool {
    !b.is_empty()
        && b != "@"
        && !b.starts_with('-')
        && !b.starts_with('/')
        && !b.ends_with('/')
        && !b.ends_with('.')
        && !b.contains("//")
        && !b.contains("..")
        && !b.contains("@{")
        && !b
            .chars()
            .any(|c| c.is_ascii_control() || " ~^:?*[\\".contains(c))
        && b.split('/')
            .all(|c| !c.starts_with('.') && !c.ends_with(".lock"))
}

/// The host part git (and ssh) would see, brackets and user removed.
fn host_of(url: &str) -> Option<&str> {
    let authority = match url.split_once("://") {
        Some((_, rest)) => rest.split('/').next()?,
        None => url.split('/').next()?.split_once(':')?.0,
    };
    let host = authority.rsplit('@').next()?;
    Some(host.trim_start_matches('['))
}

fn check_url(u: &str) {
    if check_remote_url(u).is_ok() {
        assert!(!u.starts_with('-'), "{u:?}");
        assert!(!u.chars().any(char::is_control), "{u:?}");
        assert!(
            !u.split('/').next().unwrap_or("").contains("::") || u.contains("://"),
            "{u:?}"
        );
        if let Some(h) = host_of(u) {
            assert!(
                !h.starts_with('-'),
                "host {h:?} of accepted {u:?} reads as an option"
            );
        }
    }
    if let Some(h) = url_host(u) {
        assert!(!h.is_empty(), "{u:?}");
        assert!(!h.contains(['/', '@']), "host {h:?} of {u:?}");
        assert_eq!(h, h.to_ascii_lowercase());
    }
}

fuzz_target!(|s: &str| {
    check_url(s);
    if check_branch_name(s).is_ok() {
        assert!(git_accepts_branch(s), "git refuses branch {s:?}");
    }
    if let Ok(t) = parse_target(s) {
        match &t.kind {
            TargetKind::Branch(b) => assert!(git_accepts_branch(b), "branch {b:?} from {s:?}"),
            TargetKind::BranchOrCommit(h) => {
                assert!(is_hex_commit(h) && git_accepts_branch(h), "{h:?}")
            }
            TargetKind::Commit(c) => {
                assert!(
                    !c.is_empty() && !c.starts_with('-'),
                    "commit {c:?} from {s:?}"
                )
            }
            TargetKind::PullRequest(n) => assert!(*n > 0),
            TargetKind::RefPath(r) => {
                // Resolved against a remote whose branch is the first part.
                let first = r.split('/').next().unwrap_or("").to_string();
                if let Ok(m) = resolve_ref_path(r, &[first]) {
                    let (RefPathMatch::Branch { path, .. } | RefPathMatch::Commit { path, .. }) = m;
                    if let Some(p) = path {
                        assert!(
                            !p.starts_with('/') && !p.split('/').any(|c| c == ".."),
                            "path {p:?} from {s:?} leaves the repository"
                        );
                    }
                }
            }
            TargetKind::Default => {}
        }
        if let Some(u) = &t.clone_url {
            assert!(
                check_remote_url(u).is_ok(),
                "clone URL {u:?} from {s:?} is refused"
            );
        }
        if let Some(p) = &t.path {
            assert!(
                !p.starts_with('/') && !p.split('/').any(|c| c == ".."),
                "path {p:?} from {s:?} leaves the repository"
            );
        }
        if let Some(l) = t.line {
            assert!(l > 0, "line 0 from {s:?}");
        }
        if let Some(b) = &t.base {
            assert!(!b.starts_with('-'), "base {b:?} from {s:?}");
        }
        if let Some(r) = &t.repo {
            assert_eq!(
                RepoRef::parse(&r.to_string()).ok().as_ref(),
                Some(r),
                "{s:?}"
            );
        }
    }
    if let Ok((r, url)) = parse_repo(s) {
        assert_eq!(
            RepoRef::parse(&r.to_string()).ok().as_ref(),
            Some(&r),
            "{s:?}"
        );
        if let Some(u) = url {
            assert!(
                check_remote_url(&u).is_ok(),
                "clone URL {u:?} from {s:?} is refused"
            );
        }
    }
});
