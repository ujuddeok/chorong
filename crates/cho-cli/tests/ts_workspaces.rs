//! End to end on the committed TypeScript workspace fixtures (`fixtures/ts-workspaces/`,
//! `docs/typescript-workspaces.md`): the real `cho` binary against a copy of each, with
//! its committed index seeded into a private cache. Needs no indexer, Node or network.

use cho_fixtures::TsWorkspace;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Env {
    base: PathBuf,
    root: PathBuf,
}

impl Env {
    fn new(w: TsWorkspace) -> Self {
        let base =
            std::env::temp_dir().join(format!("cho-cli-ts-{}-{}", w.name(), std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let root = w.seed_cache(&base.join("ws"), &base.join("cache")).unwrap();
        Self { base, root }
    }

    fn cho(&self, args: &[&str]) -> (i32, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_cho"))
            .arg("--json")
            .args(args)
            .arg("--root")
            .arg(&self.root)
            .env("CHORONG_CACHE_DIR", self.base.join("cache"))
            .output()
            .expect("cho runs");
        let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "bad json from {args:?}: {e}\n{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap(), json)
    }

    /// `LINE:COL` (one-based, UTF-8 bytes) of the first `needle` in `path`.
    fn at(&self, path: &str, needle: &str) -> String {
        let text = cho_core::fs::read_source_text_in(&self.root, Path::new(path)).unwrap();
        let byte = text.find(needle).unwrap_or_else(|| panic!("{needle}"));
        let line = text[..byte].matches('\n').count() + 1;
        let col = byte - text[..byte].rfind('\n').map_or(0, |i| i + 1) + 1;
        format!("{line}:{col}")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).ok();
    }
}

#[test]
fn dist_layout_reaches_source_through_mapped_links() {
    let env = Env::new(TsWorkspace::Dist);
    let main = "packages/app/src/main.ts";
    let pos = env.at(main, "greet('x')");

    let (code, status) = env.cho(&["index", "status"]);
    assert_eq!((code, status["status"].as_str()), (0, Some("current")));

    let (code, d) = env.cho(&["definition", main, &pos]);
    assert_eq!(code, 0, "{d}");
    let def = &d["definitions"][0];
    assert_eq!(def["path"], "packages/swap/src/greet.ts");
    assert_eq!(def["mapped"]["method"], "path-swap");
    assert_eq!(def["mapped"]["verified"], false);
    assert_eq!(def["mapped"]["label"], "mapped (unverified): path swap");

    let (code, r) = env.cho(&["references", main, &pos]);
    assert_eq!(code, 0);
    let paths: Vec<&str> = r["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"packages/swap/src/greet.ts"));
    assert!(paths.contains(&main));

    let (code, h) = env.cho(&["hover", main, &pos]);
    assert_eq!(code, 0);
    assert!(
        h["signature"]["text"]
            .as_str()
            .unwrap()
            .contains("greet(name: string): string"),
        "{h}"
    );
    assert_eq!(h["mapped"]["method"], "path-swap");

    // A name declared only in dist: no definition, never a guess.
    let (code, d) = env.cho(&["definition", main, &env.at(main, "Missing)")]);
    assert_eq!((code, d["found"].as_bool()), (1, Some(false)));

    let (code, g) = env.cho(&["graph", "--symbol", "greet", "--in", "--depth", "1"]);
    assert_eq!(code, 0, "{g}");
    assert!(
        g["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["status"] == "mapped" && e["to"].as_str().unwrap().contains("greet")),
        "{g}"
    );
}

#[test]
fn source_condition_layout_lands_on_source_directly() {
    let env = Env::new(TsWorkspace::SourceCondition);
    let main = "apps/web/src/main.ts";
    let pos = env.at(main, "openSession('a')");

    let (code, d) = env.cho(&["definition", main, &pos]);
    assert_eq!(code, 0, "{d}");
    let def = &d["definitions"][0];
    assert_eq!(def["path"], "packages/core/src/session.ts");
    assert!(def.get("mapped").is_none(), "{def}");

    let (code, r) = env.cho(&["references", main, &pos]);
    assert_eq!(code, 0);
    let refs = r["references"].as_array().unwrap();
    assert!(refs.iter().all(|l| l.get("mapped").is_none()));
    for p in [
        "packages/core/src/session.ts",
        "packages/client/src/index.ts",
        main,
    ] {
        assert!(refs.iter().any(|l| l["path"] == p), "{p} in {r}");
    }

    let (code, g) = env.cho(&["graph", "--symbol", "openSession", "--in", "--depth", "1"]);
    assert_eq!(code, 0, "{g}");
    let edges = g["edges"].as_array().unwrap();
    assert!(!edges.is_empty());
    assert!(edges.iter().all(|e| e["status"] == "verified"), "{g}");
}
