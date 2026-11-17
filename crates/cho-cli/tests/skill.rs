//! The investigation skill can't drift from the format: every JSON example in it is checked
//! with `cho investigation check` against `tests/fixtures/skill-examples`, and the copy in
//! `.claude/skills/` is what `cho skill investigation --install` writes.
#![allow(clippy::disallowed_methods)] // tests read their own fixtures and Chorong's output

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cho(args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_cho"))
        .arg("--json")
        .args(args)
        .output()
        .expect("cho runs");
    let json = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), json)
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn skill() -> String {
    let (code, json) = cho(&["skill", "investigation"]);
    assert_eq!(code, 0, "{json}");
    json["content"].as_str().unwrap().to_string()
}

/// Every fenced block whose info string starts with `json` (`json`, `jsonc`…).
fn json_blocks(text: &str) -> Vec<String> {
    let mut blocks = vec![];
    let mut current: Option<String> = None;
    for line in text.lines() {
        match &mut current {
            None => {
                if let Some(info) = line.trim_start().strip_prefix("```")
                    && info.trim().starts_with("json")
                {
                    current = Some(String::new());
                }
            }
            Some(block) => {
                if line.trim_start().starts_with("```") {
                    blocks.push(current.take().unwrap());
                } else {
                    block.push_str(line);
                    block.push('\n');
                }
            }
        }
    }
    assert!(current.is_none(), "unclosed code block in the skill");
    blocks
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(name: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("cho-cli-skill-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let dest = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).unwrap();
        }
    }
}

/// Check notes that are information, not problems to fix.
const NOTES: &[&str] = &["same code; differs by values"];

fn messages(report: &Value) -> Vec<String> {
    let mut out = vec![];
    let mut collect = |v: &Value| {
        if let Some(list) = v.as_array() {
            out.extend(list.iter().filter_map(|m| m.as_str().map(String::from)));
        }
    };
    collect(&report["messages"]);
    for list in ["nodes", "edges", "views"] {
        for item in report[list].as_array().into_iter().flatten() {
            collect(&item["messages"]);
        }
    }
    for row in report["comparison"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
    {
        collect(&row["messages"]);
    }
    out.retain(|m| !NOTES.iter().any(|n| m.ends_with(n)));
    out
}

#[test]
fn every_json_example_in_the_skill_checks_clean() {
    let skill = skill();
    let dir = temp_dir("examples");
    // Diff panes ask git about `main`: check against a repository made from the fixture.
    let fixture = dir.0.join("repo");
    copy_dir(
        &manifest_dir().join("tests/fixtures/skill-examples"),
        &fixture,
    );
    cho_core::test_repo::init(&fixture);
    cho_core::test_repo::commit_all(&fixture, "fixture");
    let blocks = json_blocks(&skill);
    let mut kinds = vec![];
    for (i, block) in blocks.iter().enumerate() {
        let parsed: Value = serde_json::from_str(block)
            .unwrap_or_else(|e| panic!("example {i} is not JSON: {e}\n{block}"));
        assert_eq!(
            parsed["chorong"],
            cho_investigation::FORMAT_VERSION,
            "example {i} writes the skill's format version"
        );
        kinds.push(parsed["kind"].as_str().unwrap().to_string());
        let file = dir.0.join(format!("example-{i}.json"));
        std::fs::write(&file, block).unwrap();
        let (code, report) = cho(&[
            "investigation",
            "check",
            file.to_str().unwrap(),
            "--root",
            fixture.to_str().unwrap(),
            "--no-index",
        ]);
        assert_eq!(code, 0, "example {i} has a broken anchor: {report}");
        for node in report["nodes"].as_array().unwrap() {
            assert_eq!(node["status"], "resolved", "example {i}: {node}");
        }
        for edge in report["edges"].as_array().into_iter().flatten() {
            assert_ne!(edge["verification"], "broken", "example {i}: {edge}");
        }
        for view in report["views"].as_array().into_iter().flatten() {
            assert_eq!(view["default"], false, "example {i}: {view}");
        }
        let messages = messages(&report);
        assert!(messages.is_empty(), "example {i}: {messages:?}");
    }
    let all = blocks.join("\n");
    for part in ["\"actions\"", "\"kind\": \"diff\""] {
        assert!(all.contains(part), "no example with {part}");
    }
    for kind in ["callstack", "tour", "comparison", "diagram"] {
        assert!(kinds.iter().any(|k| k == kind), "no {kind} example");
    }
}

#[test]
fn the_skill_writes_version_2_and_cho_still_reads_version_1() {
    assert_eq!(cho_investigation::FORMAT_VERSION, 2);
    let (code, json) = cho(&["formats"]);
    assert_eq!(code, 0, "{json}");
    let investigation = &json["formats"][0];
    assert_eq!(investigation["name"], "investigation");
    assert_eq!(investigation["version"], 2);
    assert_eq!(investigation["versions"], serde_json::json!([1, 2]));
    let skill = skill();
    assert!(skill.contains("write `\"chorong\": 2`"), "{skill}");
    assert!(!skill.contains("{{"), "unfilled placeholder");
}

#[test]
fn installed_skill_matches_the_copy_in_the_repository() {
    let dir = temp_dir("install");
    let (code, json) = cho(&[
        "skill",
        "investigation",
        "--install",
        dir.0.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{json}");
    let installed = std::fs::read_to_string(dir.0.join("chorong-investigations/SKILL.md")).unwrap();
    // The committed copy is generated from the master (assets/investigation-skill.md): edit
    // the master, then run the tests with CHORONG_UPDATE_DOCS=1 to rewrite the copy.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.claude/skills/chorong-investigations/SKILL.md");
    let copy = std::fs::read_to_string(&path).unwrap_or_default();
    if installed != copy && std::env::var_os("CHORONG_UPDATE_DOCS").is_some() {
        std::fs::write(&path, &installed).unwrap();
        return;
    }
    assert!(
        installed == copy,
        ".claude/skills/chorong-investigations/SKILL.md is out of date; edit \
         crates/cho-cli/assets/investigation-skill.md, then run the tests with \
         CHORONG_UPDATE_DOCS=1 to rewrite the copy"
    );
}
