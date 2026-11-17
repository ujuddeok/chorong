//! Opening against local bare repositories served over `file://`, and a local HTTP stub for
//! pull-request details. No network.
#![allow(clippy::disallowed_methods)] // tests read their own fixtures

use super::*;
use crate::auth::tests::MapStore;
use crate::testutil::{Stub, commit, file_url, git, remote};
use serde_json::json;

fn no_env(_: &str) -> Option<String> {
    None
}

struct Fixture {
    _dir: tempfile::TempDir,
    work: PathBuf,
    remote: PathBuf,
    cache: PathBuf,
    store: MapStore,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (work, remote) = remote(dir.path());
        commit(
            &work,
            &[("src/a.rs", "fn a() {}\n"), ("README", "x\n")],
            "first",
        );
        git(&work, &["push", "-q", "origin", "main"]);
        let cache = dir.path().join("cache").join("repos");
        Self {
            _dir: dir,
            work,
            remote,
            cache,
            store: MapStore::default(),
        }
    }

    fn sources(&self, api: Api) -> Sources<'_> {
        Sources {
            cache: Cache::at(&self.cache),
            git: Git::default(),
            tokens: Tokens {
                store: &self.store,
                env: &no_env,
            },
            api,
            lock_timeout: LOCK_TIMEOUT,
            index_cache: None,
        }
    }

    fn project(&self, cap: usize) -> Project {
        Project {
            url: Some(file_url(&self.remote)),
            worktree_cap: cap,
            ..Project::default()
        }
    }

    fn push(&self, refspec: &str) {
        git(&self.work, &["push", "-q", "-f", "origin", refspec]);
    }
}

fn repo() -> RepoRef {
    RepoRef::parse("github:acme/api").unwrap()
}

fn req(kind: TargetKind) -> OpenRequest {
    OpenRequest {
        kind,
        path: None,
        line: None,
        base: None,
    }
}

#[test]
fn default_branch_then_feature_branch() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    assert_eq!(o.slug, "main");
    assert_eq!(o.opened_as, OpenedAs::Branch("main".into()));
    assert_eq!(o.head.id, o.before.id);
    assert_eq!((o.behind, o.ahead), (0, 0));
    assert!(o.worktree.join("src/a.rs").is_file());
    assert!(o.worktree.starts_with(&f.cache));
    assert!(o.target_fetched_at.is_some());
    // The clone is bare and blobless.
    let bare = f.cache.join("github/acme/api.git");
    assert_eq!(
        git(&bare, &["config", "remote.origin.partialclonefilter"]),
        "blob:none"
    );

    // A feature branch, then main moves on: Before is the merge base, one behind.
    let base = git(&f.work, &["rev-parse", "HEAD"]);
    git(&f.work, &["switch", "-q", "-c", "feat/x"]);
    let head = commit(&f.work, &[("src/a.rs", "fn a() { b() }\n")], "feat");
    f.push("feat/x");
    git(&f.work, &["switch", "-q", "main"]);
    let main2 = commit(&f.work, &[("README", "y\n")], "main moves");
    f.push("main");
    let o = s
        .open(
            &repo(),
            &f.project(10),
            &req(TargetKind::Branch("feat/x".into())),
        )
        .unwrap();
    assert_eq!(o.head.id, head);
    assert_eq!(o.before.id, base);
    assert_eq!(o.target.id, main2);
    assert_eq!((o.behind, o.ahead, o.merge_bases), (1, 1, 1));
    assert_eq!(o.head.subject, "feat");
    assert!(o.slug.starts_with("feat-x~"));
    assert_eq!(
        std::fs::read_to_string(o.worktree.join("src/a.rs")).unwrap(),
        "fn a() { b() }\n"
    );

    // Re-opening the default branch moves its worktree to the new tip.
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    assert_eq!(o.head.id, main2);
    assert_eq!(
        std::fs::read_to_string(o.worktree.join("README")).unwrap(),
        "y\n"
    );

    // Before's contents are read from the object store (fetched on demand).
    let bytes = reads::read_file_at(&s.git, &bare, &base, "src/a.rs").unwrap();
    assert_eq!(bytes, b"fn a() {}\n");

    // A missing branch is an error naming it.
    let e = s
        .open(
            &repo(),
            &f.project(10),
            &req(TargetKind::Branch("nope".into())),
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("Branch `nope` not found"), "{e}");
}

#[test]
fn base_override_and_commits() {
    let f = Fixture::new();
    let first = git(&f.work, &["rev-parse", "HEAD"]);
    let second = commit(&f.work, &[("README", "2\n")], "second");
    f.push("main");
    let s = f.sources(Api::default());
    let mut r = req(TargetKind::Default);
    r.base = Some(first.clone());
    let o = s.open(&repo(), &f.project(10), &r).unwrap();
    assert_eq!(
        (o.before.id.as_str(), o.base_source),
        (first.as_str(), BaseSource::Override)
    );
    assert_eq!(o.head.id, second);

    // A commit by abbreviated id (it is in a fetched branch).
    let o = s
        .open(
            &repo(),
            &f.project(10),
            &req(TargetKind::BranchOrCommit(first[..8].into())),
        )
        .unwrap();
    assert_eq!(o.opened_as, OpenedAs::Commit(first.clone()));
    assert_eq!(o.head.id, first);
    assert_eq!(o.slug, format!("rev-{}", &first[..12]));

    // A full id that no branch contains is fetched by id.
    git(&f.work, &["switch", "-q", "-c", "tmp"]);
    let loose = commit(&f.work, &[("x", "x")], "loose");
    git(&f.work, &["push", "-q", "origin", "tmp:refs/keep/x"]);
    let o = s
        .open(
            &repo(),
            &f.project(10),
            &req(TargetKind::Commit(loose.clone())),
        )
        .unwrap();
    assert_eq!(o.head.id, loose);

    // An abbreviated id that is also a branch name: an error naming both.
    let short = first[..7].to_string();
    f.push(&format!("main:refs/heads/{short}"));
    let e = s
        .open(
            &repo(),
            &f.project(10),
            &req(TargetKind::BranchOrCommit(short.clone())),
        )
        .unwrap_err()
        .to_string();
    assert!(
        e.contains(&format!("--branch {short}")) && e.contains(&format!("--rev {short}")),
        "{e}"
    );

    let e = s
        .open(
            &repo(),
            &f.project(10),
            &req(TargetKind::Commit("0".repeat(40))),
        )
        .unwrap_err()
        .to_string();
    assert!(e.contains("not found"), "{e}");
}

#[test]
fn pull_request_with_details() {
    let f = Fixture::new();
    let base = git(&f.work, &["rev-parse", "HEAD"]);
    git(&f.work, &["switch", "-q", "-c", "release"]);
    let release = commit(&f.work, &[("R", "r")], "release");
    f.push("release");
    git(&f.work, &["switch", "-q", "-c", "feat"]);
    let head = commit(&f.work, &[("src/a.rs", "fn a() { 1 }\n")], "pr head");
    f.push("feat:refs/pull/7/head");
    let body = serde_json::json!({
        "number": 7, "title": "T", "user": {"login": "u"}, "state": "open", "merged_at": null,
        "head": {"ref": "feat", "sha": head, "repo": {"full_name": "acme/api"}},
        "base": {"ref": "release", "repo": {"full_name": "acme/api"}}
    })
    .to_string();
    let stub = Stub::start(vec![("/repos/acme/api/pulls/7", 200, body, vec![])]);
    let api = Api {
        github: stub.base.clone(),
        bitbucket: stub.base.clone(),
    };
    let s = f.sources(api);
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::PullRequest(7)))
        .unwrap();
    assert_eq!(
        (&o.opened_as, o.slug.as_str()),
        (&OpenedAs::PullRequest(7), "pr-7")
    );
    assert_eq!(o.target_branch, "release");
    assert_eq!(o.head.id, head);
    assert_eq!(o.before.id, release);
    assert_ne!(o.before.id, base);
    assert_eq!(o.pull_request.as_ref().unwrap().details.title, "T");
    assert!(o.notes.is_empty(), "{:?}", o.notes);
    // `cho open --json` keeps its shape.
    let j = serde_json::to_value(&o).unwrap();
    assert_eq!(
        (&j["kind"], &j["branch"], &j["pull_request"]["title"]),
        (&json!("pull-request"), &json!(null), &json!("T"))
    );
    assert_eq!(j["pull_request_stale"], json!(false));
    assert!(j["pull_request_fetched_at"].is_u64());
    assert!(j.get("opened_as").is_none());
    // The worktree's record says what it is, typed and in words.
    let st = RepoState::load(&s.cache.repo(&repo()).unwrap().state);
    let w = &st.worktrees["pr-7"];
    assert_eq!(
        (w.opened_as.as_ref(), w.what.as_str()),
        (Some(&OpenedAs::PullRequest(7)), "pull request 7")
    );

    // Details unavailable: still opens from refs/pull/N/head, against the default branch.
    f.push("feat:refs/pull/8/head");
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::PullRequest(8)))
        .unwrap();
    assert_eq!(o.target_branch, "main");
    assert!(
        o.notes.iter().any(|n| n.contains("could not be read")),
        "{:?}",
        o.notes
    );
    // It is still a pull request, with its number, though it has no details.
    assert_eq!(
        (&o.opened_as, o.pull_request.as_ref()),
        (&OpenedAs::PullRequest(8), None)
    );

    // No such pull request on the remote.
    let e = s
        .open(&repo(), &f.project(10), &req(TargetKind::PullRequest(99)))
        .unwrap_err()
        .to_string();
    assert!(e.contains("Pull request 99 not found"), "{e}");
}

#[test]
fn evicts_least_recently_opened_but_not_default() {
    let f = Fixture::new();
    for b in ["a", "b", "c"] {
        f.push(&format!("main:refs/heads/{b}"));
    }
    let s = f.sources(Api::default());
    let p = f.project(3);
    s.open(&repo(), &p, &req(TargetKind::Default)).unwrap();
    s.open(&repo(), &p, &req(TargetKind::Branch("a".into())))
        .unwrap();
    s.open(&repo(), &p, &req(TargetKind::Branch("b".into())))
        .unwrap();
    // Touch `a` again so `b` is the least recent.
    s.open(&repo(), &p, &req(TargetKind::Branch("a".into())))
        .unwrap();
    let o = s
        .open(&repo(), &p, &req(TargetKind::Branch("c".into())))
        .unwrap();
    assert_eq!(o.evicted.len(), 1);
    assert!(o.evicted[0].ends_with("worktrees/b"), "{:?}", o.evicted);
    let wts = f.cache.join("github/acme/api/worktrees");
    let mut names: Vec<String> = std::fs::read_dir(&wts)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["a", "c", "main"]);
    let bare = f.cache.join("github/acme/api.git");
    assert_eq!(git(&bare, &["worktree", "list"]).lines().count(), 4);

    // Removing one worktree by name.
    let removed = s.remove_worktree(&repo(), "a").unwrap();
    assert!(removed.ends_with("worktrees/a"));
    assert!(!wts.join("a").exists());
    assert!(s.remove_worktree(&repo(), "a").is_err());

    // Usage, then clearing the repository.
    let u = crate::usage::list(&s.cache);
    assert_eq!(u.len(), 1);
    assert_eq!(u[0].repo, "github:acme/api");
    assert_eq!(u[0].worktrees.len(), 2);
    assert!(u[0].clone_bytes > 0 && u[0].total_bytes > u[0].clone_bytes);
    let removed = crate::usage::clear_all(&s.cache).unwrap();
    assert_eq!(removed.len(), 2);
    assert!(crate::usage::list(&s.cache).is_empty());
}

#[test]
fn offline_uses_last_fetch() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    let first = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    assert!(!first.offline);
    std::fs::rename(&f.remote, f.remote.with_extension("gone")).unwrap();
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    assert!(o.offline);
    assert_eq!(o.head.id, first.head.id);
    assert!(
        o.notes.iter().any(|n| n.contains("Could not reach")),
        "{:?}",
        o.notes
    );
}

/// Every file under `dir` (`.git` included) with its length and modification time.
fn files_under(dir: &Path) -> std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let meta = e.path().symlink_metadata().unwrap();
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                out.insert(e.path(), (meta.len(), meta.modified().unwrap()));
            }
        }
    }
    out
}

/// Nothing outside the cache changes: not a file of the reader's checkout or of the remote,
/// `.git` included (refs, config, index, worktree lists), not just the refs.
#[test]
fn never_writes_outside_the_cache() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    let (work, remote) = (files_under(&f.work), files_under(&f.remote));
    s.open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    s.open(
        &repo(),
        &f.project(10),
        &req(TargetKind::Branch("main".into())),
    )
    .unwrap();
    assert_eq!(files_under(&f.work), work);
    assert_eq!(files_under(&f.remote), remote);
}

/// URLs git could read as options or as a remote-helper command are refused before git runs.
#[test]
fn refuses_hostile_urls() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    let marker = f.cache.parent().unwrap().join("ran");
    for url in [
        format!("--upload-pack=touch {}", marker.display()),
        format!("ext::sh -c touch% {}", marker.display()),
        "-oProxyCommand=x".to_string(),
    ] {
        let p = Project {
            url: Some(url.clone()),
            ..Project::default()
        };
        let e = s
            .open(&repo(), &p, &req(TargetKind::Default))
            .unwrap_err()
            .to_string();
        assert!(e.contains("refusing remote URL"), "{url}: {e}");
        let mut projects = crate::Projects::open(f.cache.parent().unwrap().join("p.json")).unwrap();
        assert!(projects.insert(&repo(), p).is_err());
    }
    assert!(!marker.exists());
    assert!(!f.cache.join("github/acme/api.git").exists());
}

/// Two opens of one repository at once: the second waits for the first, then succeeds.
#[test]
fn concurrent_opens_wait_for_each_other() {
    let f = Fixture::new();
    let paths = Cache::at(&f.cache).repo(&repo()).unwrap();
    let held = Cache::at(&f.cache)
        .lock(&paths, std::time::Duration::from_secs(1))
        .unwrap();
    let mut s = f.sources(Api::default());
    s.lock_timeout = std::time::Duration::from_millis(300);
    let e = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::LockBusy);
    assert!(e.to_string().contains("another Chorong"), "{e}");
    drop(held);
    let (cache, project) = (f.cache.clone(), f.project(10));
    let run = |kind: TargetKind| {
        let (cache, project) = (cache.clone(), project.clone());
        std::thread::spawn(move || {
            let store = MapStore::default();
            let s = Sources {
                cache: Cache::at(&cache),
                git: Git::default(),
                tokens: Tokens {
                    store: &store,
                    env: &no_env,
                },
                api: Api::default(),
                lock_timeout: LOCK_TIMEOUT,
                index_cache: None,
            };
            s.open(&repo(), &project, &req(kind)).map(|o| o.worktree)
        })
    };
    let a = run(TargetKind::Default);
    let b = run(TargetKind::Branch("main".into()));
    let (a, b) = (a.join().unwrap().unwrap(), b.join().unwrap().unwrap());
    assert_eq!(a, b);
}

/// Before's copies of the changed files are fetched when opening, so reading them later
/// works with the remote gone.
#[test]
fn before_contents_are_prefetched() {
    let f = Fixture::new();
    let base = git(&f.work, &["rev-parse", "HEAD"]);
    git(&f.work, &["switch", "-q", "-c", "feat"]);
    commit(&f.work, &[("src/a.rs", "fn a() { 2 }\n")], "feat");
    f.push("feat");
    let s = f.sources(Api::default());
    s.open(
        &repo(),
        &f.project(10),
        &req(TargetKind::Branch("feat".into())),
    )
    .unwrap();
    std::fs::rename(&f.remote, f.remote.with_extension("gone")).unwrap();
    let bare = f.cache.join("github/acme/api.git");
    assert_eq!(
        reads::read_file_at(&s.git, &bare, &base, "src/a.rs").unwrap(),
        b"fn a() {}\n"
    );
}

/// Prepare against an opened worktree: refused until approved, then runs once; runs again
/// only when something it depends on changes; refused outside the cache.
#[test]
fn prepare_in_an_opened_worktree() {
    use crate::prepare::{ApprovalState, Recipe, Step, approve};
    let f = Fixture::new();
    commit(
        &f.work,
        &[("package.json", "{}"), ("yarn.lock", "a")],
        "deps",
    );
    f.push("main");
    let s = f.sources(Api::default());
    let mut p = f.project(10);
    p.recipe = Some(Recipe {
        steps: vec![Step::new(&[
            "sh",
            "-c",
            "echo $GITHUB_TOKEN > out; date +%N >> runs",
        ])],
    });
    let o = s.open(&repo(), &p, &req(TargetKind::Default)).unwrap();
    let env: Vec<(std::ffi::OsString, std::ffi::OsString)> = vec![
        ("PATH".into(), std::env::var_os("PATH").unwrap()),
        ("GITHUB_TOKEN".into(), "s3cr3t".into()),
    ];
    let view = s.prepare_view(&repo(), &p, &o).unwrap();
    assert_eq!(view.approval, ApprovalState::NotApproved);
    assert_eq!(
        view.triggers.keys().cloned().collect::<Vec<_>>(),
        ["package.json", "yarn.lock"]
    );
    let e = s
        .run_prepare(&repo(), &view, &o.slug, &env, false)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not approved"), "{e}");
    assert!(!o.worktree.join("out").exists());

    p.approval = Some(approve(&view.effective, &view.triggers, &view.commit));
    let view = s.prepare_view(&repo(), &p, &o).unwrap();
    assert!(view.approval.is_approved());
    let first = s.run_prepare(&repo(), &view, &o.slug, &env, false).unwrap();
    assert!(first.ran && first.record.result == crate::prepare::run::PrepareResult::Ok);
    assert_eq!(
        std::fs::read_to_string(o.worktree.join("out")).unwrap(),
        "\n"
    );
    assert!(first.record.log.starts_with(&f.cache));
    // Nothing changed: no second run, unless forced.
    let again = s.run_prepare(&repo(), &view, &o.slug, &env, false).unwrap();
    assert!(!again.ran);
    s.run_prepare(&repo(), &view, &o.slug, &env, true).unwrap();
    assert_eq!(
        std::fs::read_to_string(o.worktree.join("runs"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    // The record survives reopening.
    let o = s.open(&repo(), &p, &req(TargetKind::Default)).unwrap();
    assert!(s.prepare_view(&repo(), &p, &o).unwrap().last_run.is_some());

    // A lockfile change on the remote: approval is needed again.
    commit(&f.work, &[("yarn.lock", "b")], "bump");
    f.push("main");
    let o = s.open(&repo(), &p, &req(TargetKind::Default)).unwrap();
    let view = s.prepare_view(&repo(), &p, &o).unwrap();
    assert!(
        matches!(&view.approval, ApprovalState::Changed { triggers, .. } if triggers.len() == 1 && triggers[0].path == "yarn.lock")
    );

    // Outside Chorong's worktrees: refused.
    let mut elsewhere = view.clone();
    elsewhere.approval = ApprovalState::Approved;
    elsewhere.worktree = f.work.clone();
    let e = s
        .run_prepare(&repo(), &elsewhere, &o.slug, &env, false)
        .unwrap_err()
        .to_string();
    assert!(e.contains("refusing to prepare"), "{e}");
    let e = s
        .run_prepare(&repo(), &elsewhere, "../../x", &env, false)
        .unwrap_err()
        .to_string();
    assert!(e.contains("refusing") || e.contains("cache"), "{e}");
}

/// Indexing permission: a cache worktree is decided only by its project approval (and
/// auto-index also needs auto-prepare); any other folder is `Local`.
#[test]
fn index_permission_both_directions() {
    use crate::prepare::ops::{Permission, assess_with};
    use crate::prepare::{approve, effective, triggers};
    let f = Fixture::new();
    commit(&f.work, &[("package.json", "{}")], "pkg");
    f.push("main");
    let s = f.sources(Api::default());
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    let cfg = f.cache.parent().unwrap().join("projects.json");
    let mut projects = crate::Projects::open(&cfg).unwrap();
    let mut p = f.project(10);
    projects.insert(&repo(), p.clone()).unwrap();
    let perm = |projects: &crate::Projects, folder: &std::path::Path| {
        assess_with(&s.cache, Ok(projects), folder).0
    };
    assert!(matches!(
        perm(&projects, &o.worktree),
        Permission::CacheNeedsApproval { .. }
    ));
    // A local folder is never decided by a project approval.
    assert_eq!(perm(&projects, &f.work), Permission::Local);

    let bare = f.cache.join("github/acme/api.git");
    let eff = effective(&s.git, &bare, &o.head.id, None).unwrap();
    let t = triggers(&s.git, &bare, &o.head.id).unwrap();
    p.approval = Some(approve(&eff, &t, &o.head.id));
    projects.insert(&repo(), p.clone()).unwrap();
    assert!(matches!(
        perm(&projects, &o.worktree),
        Permission::CacheApproved { .. }
    ));
    assert_eq!(perm(&projects, &f.work), Permission::Local);

    // package.json changes on the remote: approval needed again, naming the file.
    commit(&f.work, &[("package.json", "{\"scripts\":{}}")], "scripts");
    f.push("main");
    let o = s.open(&repo(), &p, &req(TargetKind::Default)).unwrap();
    match perm(&projects, &o.worktree) {
        Permission::CacheNeedsApproval {
            changed, reason, ..
        } => {
            assert_eq!(changed.len(), 1);
            assert!(
                reason.contains("package.json") && reason.contains("cho prepare approve"),
                "{reason}"
            );
        }
        other => panic!("{other:?}"),
    }
}

use crate::ErrorKind;
use crate::progress::{Cancel, Stage};
use crate::testutil::{closed_port_url, hanging_server};

fn cancel_after(c: &Cancel, ms: u64) -> std::thread::JoinHandle<()> {
    let c = c.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        c.cancel();
    })
}

/// Cancelling a clone that is talking to the remote stops git at once, leaves no clone
/// behind, and a later open works.
#[test]
fn cancel_during_clone_then_reopen() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    let hung = Project {
        url: Some(format!("{}/acme/api.git", hanging_server())),
        ..f.project(10)
    };
    let cancel = Cancel::new();
    let stages = std::sync::Mutex::new(vec![]);
    let t = cancel_after(&cancel, 300);
    let start = std::time::Instant::now();
    let e = s
        .open_with(
            &repo(),
            &hung,
            &req(TargetKind::Default),
            &|st| stages.lock().unwrap().push(st),
            &cancel,
        )
        .unwrap_err();
    t.join().unwrap();
    assert_eq!(e.kind, ErrorKind::Cancelled, "{e}");
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(
        stages.lock().unwrap()[..2],
        [Stage::Lock, Stage::Clone { progress: None }]
    );
    let parent = f.cache.join("github/acme");
    let left: Vec<_> = std::fs::read_dir(&parent)
        .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(
        left.iter().all(|n| n.to_string_lossy().ends_with(".lock")),
        "{left:?}"
    );
    // The lock was released and the cache is clean: a normal open succeeds.
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    assert!(o.worktree.join("src/a.rs").is_file());
}

/// Cancelled at the fetch: no ref moves and nothing is saved; reopening fetches normally.
#[test]
fn cancel_during_fetch_then_reopen() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    let first = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    let newer = commit(&f.work, &[("README", "new\n")], "newer");
    f.push("main");
    let cancel = Cancel::new();
    let seen = std::sync::Mutex::new(vec![]);
    let e = s
        .open_with(
            &repo(),
            &f.project(10),
            &req(TargetKind::Default),
            &|st| {
                if matches!(st, Stage::Fetch { progress: None }) {
                    cancel.cancel();
                }
                seen.lock().unwrap().push(st);
            },
            &cancel,
        )
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled, "{e}");
    let bare = f.cache.join("github/acme/api.git");
    assert_eq!(git(&bare, &["rev-parse", "refs/heads/main"]), first.head.id);
    assert_eq!(
        git(&first.worktree, &["rev-parse", "HEAD"]),
        first.head.id,
        "the worktree didn't move"
    );
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    assert_eq!(o.head.id, newer);
    // Every stage was reported, in order, on the successful open.
    let stages = std::sync::Mutex::new(vec![]);
    s.open_with(
        &repo(),
        &f.project(10),
        &req(TargetKind::Default),
        &|st| stages.lock().unwrap().push(std::mem::discriminant(&st)),
        &Cancel::new(),
    )
    .unwrap();
    let got = stages.into_inner().unwrap();
    for st in [
        Stage::Lock,
        Stage::ListRemote,
        Stage::Fetch { progress: None },
        Stage::MergeBase,
        Stage::Worktree { progress: None },
    ] {
        assert!(got.contains(&std::mem::discriminant(&st)), "{st:?} missing");
    }
}

/// A cancelled worktree add leaves no half-registered worktree.
#[test]
fn cancel_while_placing_a_worktree() {
    let f = Fixture::new();
    f.push("main:refs/heads/side");
    let s = f.sources(Api::default());
    s.open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    let cancel = Cancel::new();
    let e = s
        .open_with(
            &repo(),
            &f.project(10),
            &req(TargetKind::Branch("side".into())),
            &|st| {
                if matches!(st, Stage::Worktree { progress: None }) {
                    cancel.cancel();
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled);
    let bare = f.cache.join("github/acme/api.git");
    assert_eq!(git(&bare, &["worktree", "list"]).lines().count(), 2);
    assert!(!f.cache.join("github/acme/api/worktrees/side").exists());
    s.open(
        &repo(),
        &f.project(10),
        &req(TargetKind::Branch("side".into())),
    )
    .unwrap();
}

#[test]
fn error_kinds() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    // Offline: nothing listening.
    let off = Project {
        url: Some(format!("{}/x.git", closed_port_url())),
        ..f.project(10)
    };
    let e = s
        .open(&repo(), &off, &req(TargetKind::Default))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Offline, "{e}");
    // Not found: a branch, a commit, a pull request, a worktree.
    let p = f.project(10);
    for k in [
        TargetKind::Branch("nope".into()),
        TargetKind::PullRequest(99),
    ] {
        let e = s.open(&repo(), &p, &req(k.clone())).unwrap_err();
        assert_eq!(e.kind, ErrorKind::NotFound, "{k:?}: {e}");
    }
    // A missing commit says which, for a head and for a Before override alike.
    let zeros = "0".repeat(40);
    let missing = ErrorKind::CommitNotFound {
        commit: zeros.clone(),
    };
    let e = s
        .open(&repo(), &p, &req(TargetKind::Commit(zeros.clone())))
        .unwrap_err();
    assert_eq!(e.kind, missing, "{e}");
    let with_base = OpenRequest {
        base: Some(zeros.to_ascii_uppercase()),
        ..req(TargetKind::Default)
    };
    let e = s.open(&repo(), &p, &with_base).unwrap_err();
    assert_eq!(e.kind, missing, "{e}");
    assert_eq!(
        s.remove_worktree(&repo(), "nope").unwrap_err().kind,
        ErrorKind::NotFound
    );
    // Ambiguous: a branch named like a commit id.
    let first = git(&f.work, &["rev-parse", "HEAD"]);
    let short = first[..7].to_string();
    f.push(&format!("main:refs/heads/{short}"));
    let e = s
        .open(&repo(), &p, &req(TargetKind::BranchOrCommit(short.clone())))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Ambiguous { name: short });
    // Unsupported: a local folder, a page Chorong doesn't open.
    let local = RepoRef::parse("local:/tmp/x").unwrap();
    assert_eq!(
        s.open(&local, &p, &req(TargetKind::Default))
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        crate::parse_target("https://github.com/acme/api/issues/3")
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
}

/// Pull-request details are kept with the worktree; when the host can't be reached, the
/// saved copy is shown, marked stale, and still names the target branch.
#[test]
fn pull_request_details_persist() {
    let f = Fixture::new();
    git(&f.work, &["switch", "-q", "-c", "release"]);
    commit(&f.work, &[("R", "r")], "release");
    f.push("release");
    git(&f.work, &["switch", "-q", "-c", "feat"]);
    let head = commit(&f.work, &[("F", "f")], "feat");
    f.push("feat:refs/pull/7/head");
    let body = serde_json::json!({
        "number": 7, "title": "Saved", "user": {"login": "u"}, "state": "open", "merged_at": null,
        "head": {"ref": "feat", "sha": head, "repo": {"full_name": "acme/api"}},
        "base": {"ref": "release", "repo": {"full_name": "acme/api"}}
    })
    .to_string();
    let stub = Stub::start(vec![("/repos/acme/api/pulls/7", 200, body, vec![])]);
    let s = f.sources(Api {
        github: stub.base.clone(),
        bitbucket: stub.base.clone(),
    });
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::PullRequest(7)))
        .unwrap();
    assert!(!o.pull_request.as_ref().unwrap().stale);
    // The API is gone now.
    let s = f.sources(Api {
        github: closed_port_url(),
        bitbucket: closed_port_url(),
    });
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::PullRequest(7)))
        .unwrap();
    let pr = o.pull_request.as_ref().unwrap();
    assert!(pr.stale);
    assert_eq!(pr.details.title, "Saved");
    assert_eq!(o.target_branch, "release");
    assert!(
        o.notes.iter().any(|n| n.contains("could not be refreshed")),
        "{:?}",
        o.notes
    );
}

/// The "pull request updated" check: ls-remote of the head ref, no REST.
#[test]
fn pr_head_follows_the_remote() {
    let f = Fixture::new();
    git(&f.work, &["switch", "-q", "-c", "feat"]);
    let a = commit(&f.work, &[("F", "1")], "one");
    f.push("feat:refs/pull/7/head");
    // REST pointing nowhere: pr_head doesn't need it on GitHub.
    let s = f.sources(Api {
        github: closed_port_url(),
        bitbucket: closed_port_url(),
    });
    let p = f.project(10);
    let h = s.pr_head(&repo(), &p, 7).unwrap();
    assert_eq!((h.id.as_str(), h.abbreviated), (a.as_str(), false));
    assert!(h.matches(&a));
    let b = commit(&f.work, &[("F", "2")], "two");
    f.push("feat:refs/pull/7/head");
    let h = s.pr_head(&repo(), &p, 7).unwrap();
    assert!(h.matches(&b) && !h.matches(&a));
    assert_eq!(
        s.pr_head(&repo(), &p, 99).unwrap_err().kind,
        ErrorKind::NotFound
    );
}

/// Removing a project clears its cache and the indexes of its worktrees.
#[test]
fn remove_project_drops_indexes() {
    let f = Fixture::new();
    let mut s = f.sources(Api::default());
    let ix_dir = f.cache.parent().unwrap().join("indexes");
    s.index_cache = Some(cho_semantic::IndexCache::at(&ix_dir));
    let o = s
        .open(&repo(), &f.project(10), &req(TargetKind::Default))
        .unwrap();
    let canon = o.worktree.canonicalize().unwrap();
    let ix = s.index_cache.as_ref().unwrap().repo_dir(&canon);
    std::fs::create_dir_all(&ix).unwrap();
    let mut projects = crate::Projects::open(f.cache.parent().unwrap().join("p.json")).unwrap();
    projects.insert(&repo(), f.project(10)).unwrap();
    let r = s.remove_project(&mut projects, &repo()).unwrap();
    assert!(r.was_project && r.worktrees.len() == 1);
    assert!(!ix.exists() && !o.worktree.exists());
    assert!(projects.get(&repo()).is_none());
    assert!(!f.cache.join("github/acme/api.git").exists());
}

/// Branch completion straight from the remote: no clone, no cache writes beyond the root.
#[test]
fn remote_branches_without_a_clone() {
    let f = Fixture::new();
    let main = git(&f.work, &["rev-parse", "HEAD"]);
    f.push("main:refs/heads/feat/x");
    let s = f.sources(Api::default());
    let h = s
        .remote_branches_with(&repo(), &f.project(10), &Cancel::new())
        .unwrap();
    assert_eq!(h.default.as_deref(), Some("main"));
    assert_eq!(
        h.branches
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["feat/x", "main"]
    );
    assert_eq!(h.branches[1].1.to_string(), main);
    assert!(!f.cache.join("github/acme/api.git").exists());
    // Unreachable: Offline.
    let off = Project {
        url: Some(format!("{}/x.git", closed_port_url())),
        ..f.project(10)
    };
    assert_eq!(
        s.remote_branches_with(&repo(), &off, &Cancel::new())
            .unwrap_err()
            .kind,
        ErrorKind::Offline
    );
}

/// Messages are one short sentence naming the host or repository and the cause; the git
/// command and its output are in `detail`; a token is in neither.
#[test]
fn short_messages_with_detail_and_no_secrets() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    // A token in the clone URL itself (a user's own override), to a port nobody listens on.
    let port_url = closed_port_url();
    let with_token = port_url.replacen("http://", "http://x-access-token:S3CR3T-TOKEN@", 1);
    let p = Project {
        url: Some(format!("{with_token}/acme/api.git")),
        ..f.project(10)
    };
    let e = s.open(&repo(), &p, &req(TargetKind::Default)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Offline);
    // The host tried is the URL override's, and the repository is named because it differs.
    assert_eq!(
        e.message,
        "Can't reach 127.0.0.1 (connection refused) for github:acme/api"
    );
    let d = e.detail.clone().unwrap();
    assert!(d.contains("git clone"), "{d}");
    for text in [&e.message, &d] {
        assert!(!text.contains("S3CR3T"), "{text}");
    }
    // Not found, naming the branch and repository, with no command line in the message.
    let e = s
        .open(
            &repo(),
            &f.project(10),
            &req(TargetKind::Branch("nope".into())),
        )
        .unwrap_err();
    assert_eq!(e.message, "Branch `nope` not found on github:acme/api");
    // A REST refusal of a bearer token never echoes the token.
    let stub = Stub::start(vec![("/repos/acme/api/pulls/1", 401, "{}".into(), vec![])]);
    let store = MapStore::default();
    let env = |k: &str| (k == "CHORONG_GITHUB_TOKEN").then(|| "S3CR3T-TOKEN".to_string());
    let s2 = Sources {
        cache: Cache::at(&f.cache),
        git: Git::default(),
        tokens: Tokens {
            store: &store,
            env: &env,
        },
        api: Api {
            github: stub.base.clone(),
            bitbucket: stub.base.clone(),
        },
        lock_timeout: LOCK_TIMEOUT,
        index_cache: None,
    };
    let e = s2.pull_request(&repo(), None, 1).unwrap_err();
    assert_eq!(
        e.kind,
        ErrorKind::Auth {
            host: "github.com".into()
        }
    );
    assert!(!format!("{e:?}").contains("S3CR3T"), "{e:?}");
    // Offline over REST: short message, URL in detail.
    let s3 = f.sources(Api {
        github: closed_port_url(),
        bitbucket: closed_port_url(),
    });
    let e = s3.pull_request(&repo(), None, 1).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Offline);
    assert!(
        e.message.starts_with("Can't reach 127.0.0.1")
            && e.message.ends_with("for github:acme/api"),
        "{}",
        e.message
    );
    assert!(e.detail.unwrap().contains("/repos/acme/api/pulls/1"));
}

#[test]
fn hosts_of_clone_urls() {
    use super::url_host;
    for (u, h) in [
        ("https://github.com/acme/api.git", Some("github.com")),
        (
            "https://user:tok@mirror.example:8443/acme/api.git",
            Some("mirror.example"),
        ),
        (
            "ssh://git@git.example.com:7999/P/r.git",
            Some("git.example.com"),
        ),
        ("git@bitbucket.org:ws/r.git", Some("bitbucket.org")),
        ("http://[::1]:9/x.git", Some("::1")),
        ("file:///tmp/x.git", None),
        // Found by the `source_url` fuzz target: local paths with a colon have no host.
        ("dir/:x", None),
        ("/srv/a:b.git", None),
    ] {
        assert_eq!(url_host(u).as_deref(), h, "{u}");
    }
}

/// A mirror that needs credentials: the message names the mirror and the repository.
#[test]
fn auth_message_names_the_host_tried() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    let fail = crate::gitcmd::GitFailure {
        args: "clone".into(),
        stderr:
            "fatal: could not read Username for 'https://mirror.example': terminal prompts disabled"
                .into(),
        ..Default::default()
    };
    let e = s.auth_hint_for(
        &crate::Host::GitHub,
        &repo(),
        "https://mirror.example/acme/api.git",
        &fail,
    );
    assert_eq!(
        e.kind,
        ErrorKind::Auth {
            host: "mirror.example".into()
        }
    );
    assert!(
        e.message
            .starts_with("mirror.example needs a token or SSH key for github:acme/api"),
        "{}",
        e.message
    );
    // On the repository's own host, it's named the host's way.
    let e = s.auth_hint_for(
        &crate::Host::GitHub,
        &repo(),
        "https://github.com/acme/api.git",
        &fail,
    );
    assert!(
        e.message
            .starts_with("github.com needs a token or SSH key for acme/api"),
        "{}",
        e.message
    );
}

/// Found by the code-quality review: `clear(None, Some(slug))` fell through to clearing the
/// whole cache. A worktree without its repository is now refused and removes nothing.
#[test]
fn clearing_a_worktree_without_its_repository_removes_nothing() {
    let f = Fixture::new();
    let s = f.sources(Api::default());
    let marker = s.cache.root().join("keep");
    std::fs::create_dir_all(s.cache.root()).unwrap();
    std::fs::write(&marker, "x").unwrap();
    let e = s.clear(None, Some("main")).unwrap_err();
    assert!(e.message.contains("needs its repository"), "{}", e.message);
    assert!(marker.exists());
}
