use super::*;

fn kind(s: &str) -> TargetKind {
    parse_target(s).unwrap().kind
}

fn repo(s: &str) -> String {
    parse_target(s).unwrap().repo.unwrap().to_string()
}

#[test]
fn plain_inputs() {
    assert_eq!(kind(""), TargetKind::Default);
    assert_eq!(kind("#123"), TargetKind::PullRequest(123));
    assert_eq!(kind("feat/login"), TargetKind::Branch("feat/login".into()));
    assert_eq!(
        kind("3f9c1e2"),
        TargetKind::BranchOrCommit("3f9c1e2".into())
    );
    assert_eq!(
        kind("3F9C1E2"),
        TargetKind::BranchOrCommit("3f9c1e2".into())
    );
    // Six hex digits is too short to be a commit; it's a branch name.
    assert_eq!(kind("abcdef"), TargetKind::Branch("abcdef".into()));
    for bad in [
        "#0", "#x", "a b", "a..b", "-x", "x:y", "x.lock", "x.lock/y", "a/.b", "x~1",
    ] {
        assert!(parse_target(bad).is_err(), "{bad}");
    }
}

#[test]
fn github_urls() {
    let u = "https://github.com/acme/api";
    assert_eq!(repo(u), "github:acme/api");
    assert_eq!(kind(u), TargetKind::Default);
    assert_eq!(kind("https://github.com/acme/api.git"), TargetKind::Default);
    assert_eq!(
        kind("https://github.com/acme/api/tree/feat/login"),
        TargetKind::RefPath("feat/login".into())
    );
    let t = parse_target("https://github.com/acme/api/blob/main/src/lib.rs#L10-L20").unwrap();
    assert_eq!(t.kind, TargetKind::RefPath("main/src/lib.rs".into()));
    assert_eq!(t.line, Some(10));
    assert_eq!(
        kind("https://github.com/acme/api/commit/0123456789abcdef0123456789abcdef01234567"),
        TargetKind::Commit("0123456789abcdef0123456789abcdef01234567".into())
    );
    assert_eq!(
        kind("https://github.com/acme/api/pull/42/files"),
        TargetKind::PullRequest(42)
    );
    assert_eq!(
        kind("https://github.com/acme/api/tree/feat%2Fx"),
        TargetKind::RefPath("feat/x".into())
    );
    assert!(parse_target("https://github.com/acme/api/issues/3").is_err());
    assert!(parse_target("https://github.com/acme").is_err());
}

#[test]
fn bitbucket_urls() {
    let u = "https://bitbucket.org/ws/repo";
    assert_eq!(repo(u), "bitbucket:ws/repo");
    assert_eq!(kind(u), TargetKind::Default);
    assert_eq!(
        kind("https://bitbucket.org/ws/repo/branch/feat/x"),
        TargetKind::Branch("feat/x".into())
    );
    let t =
        parse_target("https://bitbucket.org/ws/repo/src/abc1234def/src/a.ts#lines-7:9").unwrap();
    assert_eq!(t.kind, TargetKind::RefPath("abc1234def/src/a.ts".into()));
    assert_eq!(t.line, Some(7));
    assert_eq!(
        kind("https://bitbucket.org/ws/repo/commits/abc1234def"),
        TargetKind::Commit("abc1234def".into())
    );
    assert_eq!(
        kind("https://bitbucket.org/ws/repo/pull-requests/9/diff"),
        TargetKind::PullRequest(9)
    );
}

#[test]
fn data_center_urls() {
    let t = parse_target("https://git.example.com/bb/projects/PRJ/repos/api/browse/src/x.rs?at=refs%2Fheads%2Ffeat%2Fy#12").unwrap();
    assert_eq!(
        repo("https://git.example.com/projects/PRJ/repos/api"),
        "bitbucket-dc:git.example.com/PRJ/api"
    );
    assert_eq!(t.kind, TargetKind::Branch("feat/y".into()));
    assert_eq!(t.path.as_deref(), Some("src/x.rs"));
    assert_eq!(t.line, Some(12));
    assert_eq!(
        t.clone_url.as_deref(),
        Some("https://git.example.com/bb/scm/PRJ/api.git")
    );
    assert_eq!(
        kind("https://git.example.com/projects/PRJ/repos/api/pull-requests/5/overview"),
        TargetKind::PullRequest(5)
    );
    assert_eq!(
        kind("https://git.example.com/projects/PRJ/repos/api/commits/abcdef0123"),
        TargetKind::Commit("abcdef0123".into())
    );
    let t = parse_target("https://git.example.com:8443/scm/PRJ/api.git").unwrap();
    assert_eq!(
        t.clone_url.as_deref(),
        Some("https://git.example.com:8443/scm/PRJ/api.git")
    );
    assert!(parse_target("https://example.com/some/page").is_err());
}

#[test]
fn clone_urls_and_links() {
    assert_eq!(repo("git@github.com:acme/api.git"), "github:acme/api");
    assert_eq!(
        repo("ssh://git@bitbucket.org/ws/repo.git"),
        "bitbucket:ws/repo"
    );
    assert_eq!(
        repo("ssh://git@git.example.com:7999/PRJ/api.git"),
        "bitbucket-dc:git.example.com/PRJ/api"
    );
    assert!(parse_target("git@gitlab.com:a/b.git").is_err());
    let t = parse_target(
        "chorong://open?repo=github:acme/api&rev=0123456789abcdef&path=src/a.rs&line=3",
    )
    .unwrap();
    assert_eq!(t.kind, TargetKind::Commit("0123456789abcdef".into()));
    assert_eq!((t.path.as_deref(), t.line), (Some("src/a.rs"), Some(3)));
    assert!(parse_target("chorong://open?repo=local:/x").is_err());
    // A pull-request link opens the pull request; `base` is Before.
    let base = "0123456789abcdef0123456789abcdef01234567";
    let t = parse_target(&format!(
        "chorong://open?repo=github:acme/api&pr=12&rev=abcdef1&base={base}&side=before&path=src/a.rs"
    ))
    .unwrap();
    assert_eq!(t.kind, TargetKind::PullRequest(12));
    assert_eq!(t.base.as_deref(), Some(base));
}

/// Found by the `source_url` fuzz target: a URL with a NUL in it was accepted and gave a
/// clone URL that git's checks refuse later.
#[test]
fn control_characters_are_refused() {
    for s in [
        "https://bitbucket.example/scm/p/r.g\0it",
        "feat/\u{1b}x",
        "git@github.com:a/b\u{7f}.git",
        "https://git.example.com:8443/scm/p/r%1Eb.git",
        "https://github.com/acme/api/tree/main/a%0Ab.rs",
    ] {
        assert!(parse_target(s).is_err(), "{s:?}");
    }
    // Found by the `source_url` fuzz target: an empty repository name was accepted.
    for s in [
        "https://github.com/aapi/.git",
        "https://bitbucket.org/ws/.git",
    ] {
        assert!(parse_target(s).is_err(), "{s:?}");
    }
    // A clone URL git's checks would refuse is refused here too.
    for s in [
        "ht@{t;::;ps:/x/scm/p/r",
        "ssh://git@[-oProxyCommand=x]:7999/p/r.git",
    ] {
        assert!(parse_target(s).is_err(), "{s:?}");
    }
}

#[test]
fn repos() {
    assert_eq!(
        parse_repo("github:acme/api").unwrap().0.to_string(),
        "github:acme/api"
    );
    assert_eq!(
        parse_repo("https://github.com/acme/api/pull/3")
            .unwrap()
            .0
            .to_string(),
        "github:acme/api"
    );
    assert!(parse_repo("feat/x").is_err());
    assert!(parse_repo("github:acme").is_err());
}

#[test]
fn ref_paths() {
    let branches = vec![
        "main".to_string(),
        "feat/x".to_string(),
        "abcdef1".to_string(),
    ];
    assert_eq!(
        resolve_ref_path("feat/x/src/a.rs", &branches).unwrap(),
        RefPathMatch::Branch {
            branch: "feat/x".into(),
            path: Some("src/a.rs".into())
        }
    );
    assert_eq!(
        resolve_ref_path("main", &branches).unwrap(),
        RefPathMatch::Branch {
            branch: "main".into(),
            path: None
        }
    );
    assert_eq!(
        resolve_ref_path("0123abcd/src", &branches).unwrap(),
        RefPathMatch::Commit {
            commit: "0123abcd".into(),
            path: Some("src".into())
        }
    );
    let e = resolve_ref_path("abcdef1/x", &branches)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("--branch abcdef1") && e.contains("--rev abcdef1"),
        "{e}"
    );
    assert!(resolve_ref_path("nope/x", &branches).is_err());
}

/// A cheap fuzz: no input panics, and every accepted branch is safe in a refspec.
#[test]
fn never_panics() {
    let pieces = [
        "",
        "#",
        "#1",
        "/",
        "//",
        "%",
        "%2",
        "%zz",
        "%2F",
        "https://",
        "github.com",
        "bitbucket.org",
        "a",
        "projects",
        "repos",
        "scm",
        "pull",
        "blob",
        "tree",
        "src",
        "browse",
        "?at=",
        "#L",
        "lines-",
        "..",
        ":",
        "@",
        "chorong://open?repo=",
        "git@",
        "0123456",
        "é",
        "\u{0}",
        " ",
    ];
    let mut seed: u64 = 0x9e3779b97f4a7c15;
    for _ in 0..20_000 {
        let mut s = String::new();
        for _ in 0..(seed % 7) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            s.push_str(pieces[(seed % pieces.len() as u64) as usize]);
        }
        seed = seed.wrapping_add(1);
        if let Ok(t) = parse_target(&s)
            && let TargetKind::Branch(b) = &t.kind
        {
            assert!(
                check_branch_name(b).is_ok() && !b.contains(':'),
                "{s:?} -> {b:?}"
            );
        }
        let _ = parse_repo(&s);
    }
}
