use super::*;
use crate::auth::TokenSource;
use crate::testutil::Stub;

#[test]
fn base64_matches_rfc4648() {
    for (i, o) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(i.as_bytes()), o);
    }
}

#[test]
fn clone_urls() {
    let p = Project::default();
    let ssh = Project {
        protocol: Protocol::Ssh,
        ..Project::default()
    };
    let gh = RepoRef::parse("github:acme/api").unwrap();
    assert_eq!(
        clone_url(&gh, &p).unwrap(),
        "https://github.com/acme/api.git"
    );
    assert_eq!(clone_url(&gh, &ssh).unwrap(), "git@github.com:acme/api.git");
    let bb = RepoRef::parse("bitbucket:ws/r").unwrap();
    assert_eq!(clone_url(&bb, &ssh).unwrap(), "git@bitbucket.org:ws/r.git");
    let dc = RepoRef::parse("bitbucket-dc:git.example.com/P/r").unwrap();
    assert_eq!(
        clone_url(&dc, &ssh).unwrap(),
        "ssh://git@git.example.com:7999/P/r.git"
    );
    let with_url = Project {
        url: Some("https://git.example.com/ctx/scm/P/r.git".into()),
        ..Project::default()
    };
    assert_eq!(
        Api::data_center(&dc, Some(&with_url)),
        "https://git.example.com/ctx"
    );
}

fn github_body(state: &str, merged: bool, fork: bool) -> String {
    serde_json::json!({
        "number": 7, "title": "Add login", "user": {"login": "someone"}, "state": state,
        "merged_at": if merged { Value::from("2026-01-01T00:00:00Z") } else { Value::Null },
        "head": {"ref": "feat/login", "sha": "0123456789abcdef0123456789abcdef01234567",
                 "repo": {"full_name": if fork { "fork/api" } else { "acme/api" }}},
        "base": {"ref": "main", "repo": {"full_name": "acme/api"}},
        "body": "ignored"
    })
    .to_string()
}

#[test]
fn github_pull_request_over_stub() {
    let stub = Stub::start(vec![
        (
            "/repos/acme/api/pulls/7",
            200,
            github_body("open", false, false),
            vec![("x-ratelimit-remaining", "59"), ("x-ratelimit-limit", "60")],
        ),
        (
            "/repos/acme/api/pulls/8",
            403,
            "{}".into(),
            vec![("x-ratelimit-remaining", "0"), ("x-ratelimit-limit", "60")],
        ),
    ]);
    let api = Api {
        github: stub.base.clone(),
        bitbucket: stub.base.clone(),
    };
    let repo = RepoRef::parse("github:acme/api").unwrap();
    let (d, rate) = pull_request(&api, &repo, None, 7, None).unwrap();
    assert_eq!(
        d,
        PrDetails {
            number: 7,
            title: "Add login".into(),
            author: "someone".into(),
            state: PrState::Open,
            source_branch: "feat/login".into(),
            target_branch: "main".into(),
            head: "0123456789abcdef0123456789abcdef01234567".into(),
            fork: false,
        }
    );
    assert_eq!(
        rate,
        Some(RateLimit {
            remaining: 59,
            limit: 60
        })
    );
    let e = pull_request(&api, &repo, None, 8, None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("cho auth add github.com"), "{e}");
    let e = pull_request(&api, &repo, None, 9, None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not found"), "{e}");
    let cred = Credential {
        token: "sekrit".into(),
        email: None,
        source: TokenSource::Env("CHORONG_GITHUB_TOKEN"),
    };
    pull_request(&api, &repo, None, 7, Some(&cred)).unwrap();
    let seen = stub.seen.lock().unwrap();
    assert_eq!(seen[0].path, "/repos/acme/api/pulls/7");
    assert_eq!(seen[0].auth, None);
    assert_eq!(seen.last().unwrap().auth.as_deref(), Some("Bearer sekrit"));
}

#[test]
fn github_states() {
    let v = |s, m, f| serde_json::from_str::<Value>(&github_body(s, m, f)).unwrap();
    assert_eq!(
        github_pr(&v("closed", true, false)).unwrap().state,
        PrState::Merged
    );
    assert_eq!(
        github_pr(&v("closed", false, false)).unwrap().state,
        PrState::Declined
    );
    assert!(github_pr(&v("open", false, true)).unwrap().fork);
}

#[test]
fn bitbucket_pull_request_over_stub() {
    let body = serde_json::json!({
        "id": 3, "title": "Fix", "state": "DECLINED", "author": {"display_name": "A Person"},
        "source": {"branch": {"name": "fix/x"}, "commit": {"hash": "abcdef012345"},
                   "repository": {"full_name": "ws/r"}},
        "destination": {"branch": {"name": "main"}, "repository": {"full_name": "ws/r"}}
    })
    .to_string();
    let stub = Stub::start(vec![(
        "/repositories/ws/r/pullrequests/3",
        200,
        body,
        vec![],
    )]);
    let api = Api {
        github: stub.base.clone(),
        bitbucket: stub.base.clone(),
    };
    let repo = RepoRef::parse("bitbucket:ws/r").unwrap();
    let cred = Credential {
        token: "tok".into(),
        email: Some("me@example.com".into()),
        source: TokenSource::Keychain,
    };
    let (d, _) = pull_request(&api, &repo, None, 3, Some(&cred)).unwrap();
    assert_eq!(
        (d.state, d.source_branch.as_str(), d.head.as_str(), d.fork),
        (PrState::Declined, "fix/x", "abcdef012345", false)
    );
    let seen = stub.seen.lock().unwrap();
    assert_eq!(
        seen[0].auth.as_deref(),
        Some(format!("Basic {}", base64(b"me@example.com:tok")).as_str())
    );
    let no_email = Credential {
        email: None,
        ..cred
    };
    assert!(pull_request(&api, &repo, None, 3, Some(&no_email)).is_err());
}

#[test]
fn data_center_fields() {
    let v: Value = serde_json::json!({
        "id": 5, "title": "T", "state": "OPEN", "author": {"user": {"name": "u"}},
        "fromRef": {"displayId": "feat/a", "latestCommit": "abc", "repository": {"id": 1}},
        "toRef": {"displayId": "main", "repository": {"id": 1}}
    });
    let d = data_center_pr(&v).unwrap();
    assert_eq!(
        (d.number, d.source_branch.as_str(), d.fork),
        (5, "feat/a", false)
    );
}

#[test]
fn token_check_over_stub() {
    let stub = Stub::start(vec![("/user", 200, "{}".into(), vec![])]);
    let api = Api {
        github: stub.base.clone(),
        bitbucket: stub.base.clone(),
    };
    let cred = Credential {
        token: "t".into(),
        email: None,
        source: TokenSource::Keychain,
    };
    assert_eq!(check_token(&api, &Host::GitHub, &cred).0, TokenCheck::Valid);
    let bad = Api {
        github: format!("{}/nope", stub.base),
        ..api
    };
    assert!(matches!(
        check_token(&bad, &Host::GitHub, &cred).0,
        TokenCheck::Unknown(_)
    ));
}

/// A redirect gets no token: the stub's `/moved` request carries no Authorization header.
#[test]
fn tokens_do_not_follow_redirects() {
    let stub = Stub::start(vec![
        (
            "/repos/acme/api/pulls/7",
            302,
            "{}".into(),
            vec![("Location", "/moved")],
        ),
        ("/moved", 200, github_body("open", false, false), vec![]),
    ]);
    let api = Api {
        github: stub.base.clone(),
        bitbucket: stub.base.clone(),
    };
    let repo = RepoRef::parse("github:acme/api").unwrap();
    let cred = Credential {
        token: "sekrit".into(),
        email: None,
        source: TokenSource::Keychain,
    };
    let _ = pull_request(&api, &repo, None, 7, Some(&cred));
    let seen = stub.seen.lock().unwrap();
    assert_eq!(seen[0].auth.as_deref(), Some("Bearer sekrit"));
    let moved: Vec<_> = seen.iter().filter(|s| s.path == "/moved").collect();
    assert!(!moved.is_empty(), "the redirect was followed: {seen:?}");
    assert!(moved.iter().all(|s| s.auth.is_none()), "{seen:?}");
}

/// A token is never sent over plain HTTP to a host that isn't loopback.
#[test]
fn no_token_over_plain_http() {
    let api = Api {
        github: "http://example.invalid".into(),
        bitbucket: "http://example.invalid".into(),
    };
    let repo = RepoRef::parse("github:acme/api").unwrap();
    let cred = Credential {
        token: "sekrit".into(),
        email: None,
        source: TokenSource::Keychain,
    };
    let e = pull_request(&api, &repo, None, 7, Some(&cred))
        .unwrap_err()
        .to_string();
    assert!(e.to_ascii_lowercase().contains("https"), "{e}");
}
