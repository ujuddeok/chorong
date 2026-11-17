//! End to end: index a tiny Cargo project with rust-analyzer, then navigate it.
//! Skips (passes) when rust-analyzer isn't installed.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn rust_analyzer_works() -> bool {
    Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct Env {
    root: PathBuf,
    cache: PathBuf,
}

impl Env {
    fn cho(&self, args: &[&str]) -> (i32, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_cho"))
            .arg("--json")
            .args(args)
            .env("CHORONG_CACHE_DIR", &self.cache)
            .output()
            .expect("cho runs");
        let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "bad json from {args:?}: {e}\n{}",
                String::from_utf8_lossy(&out.stdout)
            )
        });
        (out.status.code().unwrap(), json)
    }
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn index_navigate_and_verify_with_rust_analyzer() {
    if !rust_analyzer_works() {
        // Skipped only where tools are not required; CI (and `CHORONG_REQUIRE_TOOLS=1`) fails
        // instead, so a green run there means this test ran.
        let required = std::env::var_os("CI").is_some()
            || std::env::var_os("CHORONG_REQUIRE_TOOLS").is_some_and(|v| v != "0");
        assert!(
            !required,
            "rust-analyzer is not installed (rustup component add rust-analyzer)"
        );
        eprintln!("SKIP: rust-analyzer is not installed");
        return;
    }
    let base = std::env::temp_dir().join(format!("cho-cli-sem-{}", std::process::id()));
    std::fs::remove_dir_all(&base).ok();
    let root = base.join("proj");
    write(
        &root,
        "Cargo.toml",
        "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        &root,
        "src/lib.rs",
        "pub mod util;\n\npub fn run() -> u32 {\n    util::helper() + util::helper()\n}\n\nfn unrelated() {}\n\nfn locals() -> u32 {\n    let n = run();\n    n\n}\n",
    );
    write(&root, "src/util.rs", "pub fn helper() -> u32 {\n    1\n}\n");
    let env = Env {
        root: root.clone(),
        cache: base.join("cache"),
    };
    let r = root.to_str().unwrap();

    let (code, none) = env.cho(&["index", "status", "--root", r]);
    assert_eq!((code, none["status"].as_str()), (1, Some("none")));
    let (code, err) = env.cho(&["definition", "src/lib.rs", "4:11", "--root", r]);
    assert_eq!(code, 2, "{err}");
    assert!(err["error"].as_str().unwrap().contains("cho index"));

    let (code, built) = env.cho(&["index", "--root", r, "--only", "rust"]);
    assert_eq!(code, 0, "{built}");
    assert_eq!(built["projects"][0]["result"], "indexed");
    let (code, again) = env.cho(&["index", "--root", r]);
    assert_eq!(code, 0);
    // No Git here, so the index can't be proven current and is rebuilt; either way it's fine.
    assert!(["indexed", "up-to-date"].contains(&again["projects"][0]["result"].as_str().unwrap()));

    // `util::helper()` on line 4: column 11 is inside `helper`.
    let abs_lib = root.join("src/lib.rs");
    let (code, def) = env.cho(&["definition", abs_lib.to_str().unwrap(), "4:11", "--root", r]);
    assert_eq!(code, 0, "{def}");
    assert_eq!(def["definitions"][0]["path"], "src/util.rs");
    assert_eq!(def["definitions"][0]["line"], 1);
    assert_eq!(def["display_name"], "helper");

    let (code, refs) = env.cho(&["references", "src/util.rs", "1:8", "--root", r]);
    assert_eq!(code, 0, "{refs}");
    let list = refs["references"].as_array().unwrap();
    assert_eq!(list.len(), 3, "{refs}"); // the definition and two calls
    assert_eq!(list[0]["is_definition"], true);

    let (code, none) = env.cho(&["definition", "src/lib.rs", "7:1", "--root", r]);
    assert_eq!((code, none["found"].as_bool()), (1, Some(false)), "{none}");

    // Hover: the indexer's own signature, never a synthesised one.
    let (code, h) = env.cho(&["hover", "src/lib.rs", "4:11", "--root", r]);
    assert_eq!(code, 0, "{h}");
    assert_eq!(h["signature"]["text"], "pub fn helper() -> u32", "{h}");
    assert_eq!(h["signature"]["language"], "rust");
    assert_eq!(h["signature"]["source"], "signature-documentation");
    assert_eq!(h["no_signature"], Value::Null);
    assert_eq!(
        (h["display_name"].as_str(), h["kind"].as_str()),
        (Some("helper"), Some("Function"))
    );
    assert_eq!(
        (h["name"].as_str(), h["local_text"].as_str()),
        (Some("helper"), Some("helper"))
    );
    assert_eq!(h["definitions"][0]["path"], "src/util.rs");
    assert!(
        h["definitions"][0].get("declaration_range").is_none(),
        "only with --peek"
    );
    assert_eq!(h["local"], false);
    // --peek: the whole declaration of the definition, as byte range and lines.
    let (code, p) = env.cho(&["hover", "src/lib.rs", "4:11", "--root", r, "--peek"]);
    assert_eq!(code, 0, "{p}");
    let d = &p["definitions"][0];
    assert_eq!(d["declaration_range"]["start"], 0, "{p}");
    assert_eq!(d["declaration_range"]["end"], 32, "{p}");
    assert_eq!(
        (
            d["declaration_range"]["start_line"].as_u64(),
            d["declaration_range"]["end_line"].as_u64()
        ),
        (Some(1), Some(3))
    );
    assert_eq!(d["declaration_range"]["kind"], "declaration", "{p}");
    assert_eq!(d["declaration_range"]["marked_range"], Value::Null);
    assert_eq!(d["no_declaration"], Value::Null);
    // A local has no outline entry: the enclosing function, with its line marked.
    let (code, p) = env.cho(&["hover", "src/lib.rs", "10:9", "--root", r, "--peek"]);
    assert_eq!(code, 0, "{p}");
    let d = &p["definitions"][0]["declaration_range"];
    assert_eq!(d["kind"], "enclosing", "{p}");
    assert_eq!(
        (d["start_line"].as_u64(), d["end_line"].as_u64()),
        (Some(9), Some(12))
    );
    let m = &d["marked_range"];
    assert_eq!(
        (m["start_line"].as_u64(), m["end_line"].as_u64()),
        (Some(10), Some(10)),
        "{p}"
    );
    assert_eq!(p["definitions"][0]["no_declaration"], Value::Null);
    assert!(["current", "stale"].contains(&h["index"]["status"].as_str().unwrap()));
    // `let n = run();`: the inferred type of a local, as rust-analyzer recorded it.
    let (code, h) = env.cho(&["hover", "src/lib.rs", "10:9", "--root", r]);
    assert_eq!(code, 0, "{h}");
    assert_eq!(h["signature"]["text"], "let n: u32", "{h}");
    assert_eq!(h["local"], true);
    assert_eq!(
        (h["name"].as_str(), h["local_text"].as_str()),
        (Some("n"), Some("n"))
    );
    let (code, h) = env.cho(&["hover", "src/lib.rs", "7:1", "--root", r]);
    assert_eq!((code, h["found"].as_bool()), (1, Some(false)), "{h}");

    let (code, syms) = env.cho(&["symbols", "HELP", "--root", r]);
    assert_eq!(code, 0);
    assert_eq!(syms["symbols"][0]["display_name"], "helper");
    assert_eq!(env.cho(&["symbols", "nope_nope", "--root", r]).0, 1);

    write(
        &root,
        "inv.json",
        r#"{ "chorong": 1, "title": "t", "kind": "diagram", "repo": "local:/x", "rev": "abcdef1",
      "nodes": [
        { "id": "run", "anchor": { "path": "src/lib.rs", "symbol": "run" } },
        { "id": "helper", "anchor": { "path": "src/util.rs", "symbol": "helper" } },
        { "id": "unrelated", "anchor": { "path": "src/lib.rs", "symbol": "unrelated" } }
      ],
      "edges": [
        { "from": "run", "to": "helper", "rel": "calls" },
        { "from": "helper", "to": "run", "rel": "calls" },
        { "from": "unrelated", "to": "helper", "rel": "calls" }
      ] }"#,
    );
    let inv = root.join("inv.json");
    let inv = inv.to_str().unwrap();
    let (code, checked) = env.cho(&["investigation", "check", inv, "--root", r]);
    let edges: Vec<_> = checked["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["status"].as_str().unwrap())
        .collect();
    assert_eq!((code, edges), (0, vec!["verified", "claimed", "claimed"]));
    assert_eq!(checked["index"]["used"], true);
    let (_, off) = env.cho(&["investigation", "check", inv, "--root", r, "--no-index"]);
    assert!(
        off["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["status"] == "claimed")
    );
    assert_eq!(off["index"]["used"], false);

    std::fs::remove_dir_all(base).ok();
}

#[test]
fn navigation_without_an_index_is_an_error_not_a_guess() {
    let dir = std::env::temp_dir().join(format!("cho-cli-noidx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
    let env = Env {
        root: dir.clone(),
        cache: dir.join("cache"),
    };
    let r = dir.to_str().unwrap();
    for args in [
        vec!["definition", "a.rs", "1:4"],
        vec!["references", "a.rs", "1:4"],
        vec!["symbols", "a"],
    ] {
        let mut full = args.clone();
        full.extend(["--root", r]);
        let (code, out) = env.cho(&full);
        assert_eq!(code, 2, "{args:?} {out}");
    }
    let (code, out) = env.cho(&["definition", "a.rs", "0:0", "--root", r]);
    assert_eq!(code, 2, "{out}");
    let _ = &env.root;
    std::fs::remove_dir_all(dir).ok();
}
