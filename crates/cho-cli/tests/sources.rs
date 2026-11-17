//! Remote sources end to end (`cho project`, `cho open REPO …`, `cho cache`, `cho auth`,
//! `cho credential`) against a local bare repository over `file://`. No network: the REST
//! endpoint points at a closed local port.
#![allow(clippy::disallowed_methods)] // tests read their own fixtures

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Env {
    root: PathBuf,
}

use cho_core::test_repo::git;

impl Env {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("cho-cli-sources-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let work = root.join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        std::fs::write(work.join("src/a.rs"), "fn a() {}\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "first"]);
        let remote = root.join("remote.git");
        git(
            &root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        git(&remote, &["config", "uploadpack.allowFilter", "true"]);
        git(&work, &["push", "-q", remote.to_str().unwrap(), "main"]);
        git(&work, &["switch", "-q", "-c", "feat/x"]);
        std::fs::write(work.join("src/a.rs"), "fn a() { 1 }\n").unwrap();
        git(&work, &["commit", "-q", "-am", "feat"]);
        git(
            &work,
            &[
                "push",
                "-q",
                remote.to_str().unwrap(),
                "feat/x",
                "feat/x:refs/pull/4/head",
            ],
        );
        Env { root }
    }

    fn url(&self) -> String {
        format!("file://{}", self.root.join("remote.git").display())
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cho"));
        c.args(args)
            .env("CHORONG_CONFIG_DIR", self.root.join("cfg"))
            .env("CHORONG_REPOS_DIR", self.root.join("repos"))
            .env("CHORONG_CACHE_DIR", self.root.join("indexes"))
            .env("CHORONG_GITHUB_API_URL", "http://127.0.0.1:9")
            .env_remove("CHORONG_GITHUB_TOKEN")
            .env_remove("CHORONG_BITBUCKET_TOKEN");
        c
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let out = self.cmd(&a).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        (
            out.status.code().unwrap(),
            serde_json::from_str(&text).unwrap_or(Value::Null),
        )
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn projects_open_and_cache() {
    let e = Env::new("open");
    let (code, j) = e.json(&[
        "project",
        "add",
        "github:acme/api",
        "--url",
        &e.url(),
        "--worktree-cap",
        "3",
    ]);
    assert_eq!((code, j["added"].as_bool()), (0, Some(true)), "{j}");
    let (_, j) = e.json(&["project", "list"]);
    assert_eq!(j["projects"][0]["repo"], "github:acme/api");
    assert_eq!(j["projects"][0]["worktree_cap"], 3);

    let (code, j) = e.json(&["open", "github:acme/api", "--branch", "feat/x"]);
    assert_eq!(code, 0, "{j}");
    assert_eq!(j["kind"], "branch");
    assert_eq!(
        (j["ahead"].as_u64(), j["behind"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(j["before"]["subject"], "first");
    assert_eq!(j["head"]["subject"], "feat");
    let wt = PathBuf::from(j["worktree"].as_str().unwrap());
    assert!(wt.starts_with(e.root.join("repos")));
    // Recorded as a known folder, so chorong:// links to the repository resolve to it.
    let recent: Value = serde_json::from_str(
        &std::fs::read_to_string(e.root.join("cfg").join("recent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        recent["recent"][0].as_str().map(PathBuf::from),
        Some(wt.canonicalize().unwrap())
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("src/a.rs")).unwrap(),
        "fn a() { 1 }\n"
    );

    // A pull request whose details can't be fetched still opens from refs/pull/N/head.
    let (code, j) = e.json(&["open", "github:acme/api", "--pr", "4"]);
    assert_eq!((code, j["slug"].as_str()), (0, Some("pr-4")), "{j}");
    assert!(
        j["notes"][0]
            .as_str()
            .unwrap()
            .contains("could not be read")
    );

    // A web URL names the repository, branch and file.
    let (code, j) = e.json(&[
        "open",
        "https://github.com/acme/api/blob/feat/x/src/a.rs#L1",
    ]);
    assert_eq!(code, 0, "{j}");
    assert_eq!(
        (j["branch"].as_str(), j["path"].as_str(), j["line"].as_u64()),
        (Some("feat/x"), Some("src/a.rs"), Some(1))
    );

    // Text output names the worktree first.
    let out = e
        .cmd(&[
            "open",
            "github:acme/api",
            "--branch",
            "feat/x",
            "--base",
            "main",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.lines().next().unwrap().contains("worktrees/feat-x~"),
        "{text}"
    );
    assert!(
        text.contains("Before") && text.contains("(--base)"),
        "{text}"
    );

    // --view opens the worktree in the viewer (dry run: the launch it would make).
    let (code, j) = e.json(&[
        "open",
        "github:acme/api",
        "--branch",
        "feat/x",
        "--view",
        "--dry-run",
    ]);
    assert_eq!(code, 0, "{j}");
    assert!(
        j["viewer"]["program"]
            .as_str()
            .unwrap()
            .ends_with("chorong-app"),
        "{j}"
    );
    assert_eq!(j["viewer"]["args"][0].as_str(), j["worktree"].as_str());
    assert_eq!(j["viewer"]["launched"], false);

    // Errors: unknown branch, bad --rev.
    let (code, j) = e.json(&["open", "github:acme/api", "--branch", "nope"]);
    assert!(
        code == 2
            && j["error"] == "Branch `nope` not found on github:acme/api"
            && j["kind"] == "not-found",
        "{j}"
    );
    let (code, _) = e.json(&["open", "github:acme/api", "--rev", "xyz"]);
    assert_eq!(code, 2);

    let (_, j) = e.json(&["cache", "list"]);
    assert_eq!(j["repos"][0]["repo"], "github:acme/api");
    assert_eq!(j["repos"][0]["worktrees"].as_array().unwrap().len(), 2);
    let (code, j) = e.json(&["cache", "clear", "github:acme/api", "pr-4"]);
    assert_eq!(
        (code, j["worktrees_removed"].as_array().unwrap().len()),
        (0, 1),
        "{j}"
    );
    let (code, _) = e.json(&["project", "remove", "github:acme/api"]);
    assert_eq!(code, 0);
    assert!(!e.root.join("repos/github/acme/api.git").exists());
    let (_, j) = e.json(&["project", "list"]);
    assert_eq!(j["projects"].as_array().unwrap().len(), 0);
}

#[test]
fn auth_never_prints_the_token() {
    let e = Env::new("auth");
    let out = e
        .cmd(&["--json", "auth", "status", "github.com", "--no-check"])
        .env("CHORONG_GITHUB_TOKEN", "s3cr3t-token")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("s3cr3t") && text.contains("CHORONG_GITHUB_TOKEN"),
        "{text}"
    );
    let j: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(j["hosts"][0]["token"], true);

    // git's credential protocol: answers HTTPS requests for the host, nothing else.
    let mut child = e
        .cmd(&["credential", "get"])
        .env("CHORONG_GITHUB_TOKEN", "s3cr3t-token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=github.com\n\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "username=x-access-token\npassword=s3cr3t-token\n"
    );
}

#[test]
fn prepare_needs_approval_and_runs_without_tokens() {
    let e = Env::new("prepare");
    let (code, _) = e.json(&["project", "add", "github:acme/api", "--url", &e.url()]);
    assert_eq!(code, 0);
    let (code, j) = e.json(&["prepare", "show", "github:acme/api"]);
    assert_eq!(code, 0, "{j}");
    assert_eq!(j["approval"]["state"], "not-approved");
    assert_eq!(j["effective"]["source"], "none");

    let (code, j) = e.json(&[
        "prepare",
        "set",
        "github:acme/api",
        "--",
        "sh",
        "-c",
        "echo \"${GITHUB_TOKEN}x\" > leaked; echo ok > prepared",
    ]);
    assert_eq!(code, 0, "{j}");

    // Not approved: `open --prepare` runs nothing.
    let out = e
        .cmd(&["--json", "open", "github:acme/api", "--prepare"])
        .env("GITHUB_TOKEN", "s3cr3t")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let j: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(j["prepare"]["approved"], false, "{j}");
    let wt = PathBuf::from(j["worktree"].as_str().unwrap());
    assert!(!wt.join("prepared").exists());
    // Indexing Chorong's worktree is refused too.
    let (code, j) = e.json(&["index", "--root", wt.to_str().unwrap()]);
    assert!(
        code == 2 && j["error"].as_str().unwrap().contains("approve it first"),
        "{j}"
    );

    let (code, j) = e.json(&["prepare", "approve", "github:acme/api"]);
    assert_eq!((code, j["approved"].as_bool()), (0, Some(true)), "{j}");
    let out = e
        .cmd(&["--json", "prepare", "run", "github:acme/api"])
        .env("GITHUB_TOKEN", "s3cr3t")
        .output()
        .unwrap();
    let j: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (j["ran"].as_bool(), j["record"]["result"].as_str()),
        (Some(true), Some("ok")),
        "{j}"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("prepared")).unwrap(),
        "ok\n"
    );
    assert_eq!(std::fs::read_to_string(wt.join("leaked")).unwrap(), "x\n");
    let (_, j) = e.json(&["prepare", "run", "github:acme/api"]);
    assert_eq!(j["ran"], false);

    // Approved, but `cho open` without --prepare prepares nothing: only the flag starts it.
    std::fs::remove_file(wt.join("prepared")).unwrap();
    let (code, j) = e.json(&["open", "github:acme/api"]);
    assert_eq!(code, 0, "{j}");
    assert!(j.get("prepare").is_none(), "{j}");
    assert!(!wt.join("prepared").exists());

    // `cho prepare auto` was removed, and says so.
    let (code, j) = e.json(&["prepare", "auto", "github:acme/api", "on"]);
    assert!(
        code == 2 && j["error"].as_str().unwrap().contains("was removed"),
        "{j}"
    );
    let (code, _) = e.json(&["prepare", "clear", "github:acme/api"]);
    assert_eq!(code, 0);
}

#[test]
fn errors_are_short_with_detail_on_request() {
    let e = Env::new("errors");
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        format!("http://127.0.0.1:{p}/acme/api.git")
    };
    let (code, _) = e.json(&["project", "add", "github:acme/api", "--url", &dead]);
    assert_eq!(code, 0);
    let (code, j) = e.json(&["open", "github:acme/api"]);
    assert_eq!(code, 2, "{j}");
    assert_eq!(j["kind"], "offline");
    assert_eq!(
        j["error"], "Can't reach 127.0.0.1 (connection refused) for github:acme/api",
        "{j}"
    );
    assert!(!j["error"].as_str().unwrap().contains("git clone"));
    assert!(j["detail"].as_str().unwrap().contains("git clone"), "{j}");
    // Text: short line, and the detail only with --verbose.
    let out = e.cmd(&["open", "github:acme/api"]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--verbose") && !err.contains("git clone"),
        "{err}"
    );
    let out = e
        .cmd(&["--verbose", "open", "github:acme/api"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("git clone"));
}
