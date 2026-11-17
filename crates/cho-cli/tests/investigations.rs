//! `cho investigation list` and `check --at`, end to end, on a throwaway git repository.

use cho_core::test_repo;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cho(args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cho"))
        .arg("--json")
        .args(args)
        .output()
        .expect("cho runs");
    let json = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), json)
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A repository whose first commit has `beta` and whose HEAD renamed it to `gamma`, with a
/// tour pinned to the first commit in `.chorong/investigations/`.
fn repo(name: &str) -> (Repo, String) {
    let root = std::env::temp_dir().join(format!("cho-cli-inv-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    test_repo::init(&root);
    write(&root, "src/a.rs", "fn alpha() {}\nfn beta() {}\n");
    let first = test_repo::commit_all(&root, "one");
    write(&root, "src/a.rs", "fn alpha() {}\nfn gamma() {}\n");
    test_repo::commit_all(&root, "two");
    let tour = serde_json::json!({
        "chorong": 1, "title": "Alpha and beta", "kind": "tour", "repo": "local:/x", "rev": first,
        "nodes": [
            { "id": "a", "anchor": { "path": "src/a.rs", "symbol": "alpha" } },
            { "id": "b", "anchor": { "path": "src/a.rs", "symbol": "beta" } }
        ],
        "steps": ["a", "b"]
    });
    write(
        &root,
        ".chorong/investigations/tour.json",
        &tour.to_string(),
    );
    write(&root, ".chorong/investigations/bad.json", "{}");
    (Repo(root), first)
}

#[test]
fn list_shows_title_kind_and_rev_or_a_reason() {
    let (repo, first) = repo("list");
    let r = repo.0.to_str().unwrap();
    let (code, json) = cho(&["investigation", "list", "--root", r]);
    assert_eq!(code, 0, "{json}");
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["path"], ".chorong/investigations/bad.json");
    assert!(entries[0]["reason"].is_string());
    assert_eq!(entries[1]["title"], "Alpha and beta");
    assert_eq!(entries[1]["kind"], "tour");
    assert_eq!(entries[1]["rev"], first);
}

#[test]
fn check_at_rev_reads_the_pinned_commit() {
    let (repo, first) = repo("check");
    let r = repo.0.to_str().unwrap();
    let file = repo.0.join(".chorong/investigations/tour.json");
    let f = file.to_str().unwrap();

    let (code, json) = cho(&["investigation", "check", f, "--root", r, "--no-index"]);
    assert_eq!(code, 1, "beta is gone from the working tree: {json}");
    assert_eq!(json["at"], "worktree");
    assert_eq!(json["drift"]["state"], "differs");
    assert_eq!(json["drift"]["rev_present"], true);
    assert_eq!(json["source"]["path"], f);

    let (code, json) = cho(&["investigation", "check", f, "--root", r, "--at", "rev"]);
    assert_eq!(code, 0, "{json}");
    assert_eq!(json["at"], "rev");
    assert_eq!(json["nodes"][1]["status"], "resolved");
    assert_eq!(json["index"]["used"], false);

    // A pinned commit that isn't in the repository is an error, not a guess.
    let text = cho_core::fs::read_source_text(&file)
        .unwrap()
        .replace(&first, "deadbeefdeadbeef");
    write(&repo.0, "other.json", &text);
    let other = repo.0.join("other.json");
    let (code, json) = cho(&[
        "investigation",
        "check",
        other.to_str().unwrap(),
        "--root",
        r,
        "--at",
        "rev",
    ]);
    assert_eq!(code, 2, "{json}");
    assert!(json["error"].as_str().unwrap().contains("deadbeef"));
}

#[test]
fn check_refuses_non_https_urls() {
    for url in ["http://example.com/t.json", "file:///etc/passwd"] {
        let (code, json) = cho(&["investigation", "check", url]);
        assert_eq!(code, 2, "{json}");
        assert!(json["error"].as_str().unwrap().contains("https"), "{json}");
    }
}

#[test]
fn check_reports_version_two_parts() {
    let (repo, first) = repo("v2");
    let r = repo.0.to_str().unwrap();
    let inv = serde_json::json!({
        "chorong": 2, "title": "v2", "kind": "tour", "repo": "local:/x", "rev": first,
        "nodes": [
            { "id": "a", "name": "Start", "evidence": "read",
              "anchor": { "path": "src/a.rs", "symbol": "alpha", "from": "fn", "to": "{}" } }
        ],
        "externals": [ { "id": "db", "what": "database", "label": "DB" } ],
        "edges": [ { "from": "a", "to": "db", "rel": "calls" } ],
        "groups": [ { "label": "Setup", "members": ["a"], "phase": "setup" } ],
        "steps": ["a", "db"],
        "views": { "a": { "panes": [
            { "kind": "editor", "highlights": [ { "anchor": "a", "note": "here" } ] },
            { "kind": "wormhole" }
        ] } }
    });
    let file = repo.0.join("v2.json");
    std::fs::write(&file, inv.to_string()).unwrap();
    let f = file.to_str().unwrap();
    let (code, json) = cho(&["investigation", "check", f, "--root", r, "--no-index"]);
    assert_eq!(code, 0, "{json}");
    assert_eq!(json["nodes"][0]["author_evidence"], "read");
    assert_eq!(json["steps"][0]["number"], "S1");
    assert_eq!(json["steps"][1]["external"], true);
    assert_eq!(json["externals"][0]["status"], "not-in-code");
    assert_eq!(
        json["views"][0]["default"], true,
        "unknown pane kind falls back"
    );
    assert!(json["messages"][0].as_str().unwrap().contains("wormhole"));
    assert_eq!(json["edges"][0]["status"], "claimed");

    // The same fields in a version 1 file are rejected.
    let mut v1 = inv.clone();
    v1["chorong"] = 1.into();
    std::fs::write(&file, v1.to_string()).unwrap();
    let (code, _) = cho(&["investigation", "check", f, "--root", r, "--no-index"]);
    assert_ne!(code, 0);
}

#[test]
fn check_reports_action_steps_and_diff_panes() {
    let (repo, first) = repo("actions");
    let r = repo.0.to_str().unwrap();
    // The working tree changes `src/a.rs` against `main`.
    write(&repo.0, "src/a.rs", "fn alpha() {}\nfn delta() {}\n");
    let inv = |base: &str| {
        serde_json::json!({
            "chorong": 2, "title": "Mixed", "kind": "tour", "repo": "local:/x", "rev": first,
            "nodes": [ { "id": "a", "anchor": { "path": "src/a.rs", "symbol": "alpha" } } ],
            "actions": [ { "id": "run", "name": "Run it", "note": "Run the **migration**.",
                           "commands": [ { "label": "Apply", "text": "make migrate" } ] } ],
            "steps": ["a", "run"],
            "views": {
                "a": { "panes": [ { "kind": "diff", "path": "src/a.rs", "base": base } ] },
                "run": { "panes": [ { "kind": "editor", "anchor": "a" } ] }
            }
        })
    };
    let file = repo.0.join("mixed.json");
    let f = file.to_str().unwrap();
    std::fs::write(&file, inv("main").to_string()).unwrap();
    let (code, json) = cho(&["investigation", "check", f, "--root", r, "--no-index"]);
    assert_eq!(code, 0, "{json}");
    assert_eq!(json["actions"][0]["status"], "action");
    assert_eq!(json["actions"][0]["commands"], 1);
    assert_eq!(json["steps"][1]["action"], true);
    assert!(json["steps"][1].get("depth").is_none(), "{json}");
    let diff = &json["views"][0]["panes"][0];
    assert_eq!(diff["kind"], "diff", "{json}");
    assert_eq!(diff["base"]["has_file"], true);
    assert!(diff["head"].get("rev").is_none(), "the working tree");
    assert!(diff.get("problem").is_none(), "{json}");

    // An unresolved base is reported and makes the check fail; nothing is guessed.
    std::fs::write(&file, inv("no-such-branch").to_string()).unwrap();
    let (code, json) = cho(&["investigation", "check", f, "--root", r, "--no-index"]);
    assert_ne!(code, 0, "{json}");
    let problem = json["views"][0]["panes"][0]["problem"].as_str().unwrap();
    assert!(problem.contains("no-such-branch"), "{problem}");

    // A revision that looks like an option is refused when the file is read.
    std::fs::write(&file, inv("--all").to_string()).unwrap();
    let (code, json) = cho(&["investigation", "check", f, "--root", r, "--no-index"]);
    assert_eq!(code, 2, "{json}");

    // Edges never touch an action.
    let mut bad = inv("main");
    bad["edges"] = serde_json::json!([ { "from": "a", "to": "run", "rel": "calls" } ]);
    std::fs::write(&file, bad.to_string()).unwrap();
    let (code, json) = cho(&["investigation", "check", f, "--root", r, "--no-index"]);
    assert_eq!(code, 2, "{json}");
    assert!(json["error"].as_str().unwrap().contains("action"), "{json}");
}
