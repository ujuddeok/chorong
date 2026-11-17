//! `cho settings` end to end, against a throwaway config directory.
#![allow(clippy::disallowed_methods)] // tests read their own fixtures

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Env {
    root: PathBuf,
}

impl Env {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("cho-cli-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        Env { root }
    }
    fn cfg(&self) -> PathBuf {
        self.root.join("cfg")
    }
    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }
    fn run(&self, args: &[&str]) -> (i32, String, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_cho"))
            .args(args)
            .env("CHORONG_CONFIG_DIR", self.cfg())
            .output()
            .expect("cho runs");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let json = serde_json::from_str(&text).unwrap_or(Value::Null);
        (out.status.code().unwrap(), text, json)
    }
    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let (code, _, json) = self.run(&a);
        (code, json)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn setting<'a>(json: &'a Value, key: &str) -> &'a Value {
    json["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap()
}

#[test]
fn lists_defaults_with_metadata() {
    let e = Env::new("list");
    let (code, json) = e.json(&["settings"]);
    assert_eq!(code, 0);
    assert_eq!(json["lightweight"], false);
    assert!(json["repo"].is_null());
    assert_eq!(json["files"]["settings"]["status"], "missing");
    let s = setting(&json, "changes.auto_refresh");
    assert_eq!(s["value"], true);
    assert_eq!(s["source"], "default");
    assert_eq!(s["cost_class"], "background-heavy");
    assert_eq!(s["lightweight_off"], true);
    assert!(
        s["description"].as_str().unwrap().len() > 5 && s["cost_hint"].as_str().unwrap().len() > 5
    );
    assert_eq!(setting(&json, "search.max_results")["kind"]["min"], 100);
    // Text form.
    let (code, text, _) = e.run(&["settings"]);
    assert_eq!(code, 0);
    assert!(text.contains("context.follow_hover") && text.contains("default"));
}

#[test]
fn set_get_unset_global() {
    let e = Env::new("global");
    let (code, text, _) = e.run(&["settings", "set", "hover.card", "false"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("hover.card = false (global)"));
    assert!(e.cfg().join("settings.json").exists());

    let (_, text, _) = e.run(&["settings", "get", "hover.card"]);
    assert_eq!(text.trim(), "false");
    let (_, json) = e.json(&["settings", "get", "hover.card"]);
    assert_eq!(
        (json["source"].clone(), json["global"].clone()),
        ("global".into(), false.into())
    );

    let (_, json) = e.json(&["settings", "set", "search.max_results", "250"]);
    assert_eq!(json["value"], 250);
    assert_eq!(json["scope"], "global");
    assert_eq!(json["changed"], true);

    let (code, json) = e.json(&["settings", "unset", "hover.card"]);
    assert_eq!(code, 0);
    assert_eq!(json["source"], "default");
}

#[test]
fn repo_override_and_lightweight_resolution() {
    let e = Env::new("repo");
    let repo = e.repo();
    let r = repo.to_str().unwrap();
    e.run(&["settings", "set", "index.auto_refresh", "false"]);
    let (code, json) = e.json(&["settings", "set", "index.auto_refresh", "true", "--repo", r]);
    assert_eq!(code, 0);
    assert_eq!(
        (json["scope"].clone(), json["source"].clone()),
        ("repo".into(), "repo".into())
    );
    assert!(!repo.join("settings.json").exists() && std::fs::read_dir(&repo).unwrap().count() == 0);

    let (_, json) = e.json(&["settings", "--repo", r]);
    assert_eq!(setting(&json, "index.auto_refresh")["source"], "repo");
    assert_eq!(
        json["repo"],
        Path::new(r).canonicalize().unwrap().to_str().unwrap()
    );
    // Without --repo the override doesn't apply.
    let (_, json) = e.json(&["settings"]);
    assert_eq!(setting(&json, "index.auto_refresh")["source"], "global");

    // Lightweight mode beats the repo override, and says what it covered up.
    e.run(&["settings", "set", "lightweight", "true"]);
    let (_, json) = e.json(&["settings", "--repo", r]);
    let s = setting(&json, "index.auto_refresh");
    assert_eq!(
        (s["value"].clone(), s["source"].clone()),
        (false.into(), "lightweight".into())
    );
    assert_eq!(s["lightweight_overrode"]["would_be"], true);
    assert_eq!(s["lightweight_overrode"]["from"], "repo");
    assert_eq!(json["lightweight"], true);

    e.run(&["settings", "set", "lightweight", "false"]);
    let (_, json) = e.json(&["settings", "get", "index.auto_refresh", "--repo", r]);
    assert_eq!(json["value"], true);
}

/// Retired keys (manual start) say so instead of "unknown setting".
#[test]
fn retired_keys_say_why() {
    let e = Env::new("retired");
    for key in ["index.auto_build", "prepare.auto"] {
        let (code, json) = e.json(&["settings", "set", key, "true"]);
        assert_eq!(code, 2, "{json}");
        assert!(
            json["error"].as_str().unwrap().contains("was retired"),
            "{json}"
        );
    }
}

#[test]
fn errors_exit_two_and_write_nothing() {
    let e = Env::new("errors");
    for args in [
        vec!["settings", "set", "nope", "true"],
        vec!["settings", "set", "hover.card", "maybe"],
        vec!["settings", "set", "search.max_results", "3"],
        vec!["settings", "set", "search.max_results", "-1"],
        vec!["settings", "get", "nope"],
        vec![
            "settings",
            "set",
            "hover.card",
            "false",
            "--repo",
            "/no/such/dir",
        ],
    ] {
        let (code, json) = e.json(&args);
        assert_eq!(code, 2, "{args:?}");
        assert!(json["error"].is_string(), "{args:?}: {json}");
    }
    assert!(!e.cfg().exists());
}

#[test]
fn malformed_file_is_reported_not_overwritten() {
    let e = Env::new("malformed");
    std::fs::create_dir_all(e.cfg()).unwrap();
    std::fs::write(e.cfg().join("settings.json"), "{ \"hover.card\": ").unwrap();
    let (code, json) = e.json(&["settings"]);
    assert_eq!(code, 1);
    assert_eq!(json["files"]["settings"]["status"], "malformed");
    assert_eq!(json["diagnostics"][0]["kind"], "malformed");
    // Settings still resolve (to defaults here).
    assert_eq!(setting(&json, "hover.card")["value"], true);
    let (code, json) = e.json(&["settings", "set", "hover.card", "false"]);
    assert_eq!(code, 2);
    assert!(json["error"].as_str().unwrap().contains("syntax error"));
    assert_eq!(
        std::fs::read_to_string(e.cfg().join("settings.json")).unwrap(),
        "{ \"hover.card\": "
    );
}

#[test]
fn unknown_keys_and_comments_survive_set() {
    let e = Env::new("unknown");
    std::fs::create_dir_all(e.cfg()).unwrap();
    let text = "// mine\n{\n  \"future.key\": 7, // newer cho\n  \"hover.card\": true\n}\n";
    std::fs::write(e.cfg().join("settings.json"), text).unwrap();
    let (_, json) = e.json(&["settings"]);
    assert_eq!(json["unknown_keys"][0]["key"], "future.key");
    e.run(&["settings", "set", "hover.card", "false"]);
    let after = std::fs::read_to_string(e.cfg().join("settings.json")).unwrap();
    assert_eq!(
        after,
        text.replace("\"hover.card\": true", "\"hover.card\": false")
    );
}
