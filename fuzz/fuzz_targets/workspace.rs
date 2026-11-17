#![no_main]
//! The workspace readers behind the dist→source join: tsconfig JSONC, package.json
//! entries, path normalising, workspace globs, pnpm-workspace.yaml, `export … from`
//! statements and npm SCIP symbols. Repositories write all of these.
use libfuzzer_sys::fuzz_target;
use cho_core::jsonc::{MAX_DEPTH, parse_value};
use cho_semantic::dist_link::{parse_symbol, reexports};
use cho_semantic::relpath::normalize;
use cho_semantic::workspace::{
    entries, glob_match, is_declaration_file, pnpm_packages,
};
use serde_json::Value;

/// How deeply `v` nests arrays and objects (a scalar is 0).
fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// `Some(n)` is relative, has no empty, `.` or `..` segments, and normalises to itself.
fn check_normal(p: &str) {
    if let Some(n) = normalize(p) {
        assert!(!n.starts_with('/'), "{p:?} -> {n:?}");
        assert!(
            n.is_empty() || n.split('/').all(|s| !s.is_empty() && s != "." && s != ".."),
            "{p:?} -> {n:?}"
        );
        assert_eq!(
            normalize(&n).as_deref(),
            Some(n.as_str()),
            "not idempotent: {p:?}"
        );
    }
}

/// Pretty JSON with a comment at every line end and a trailing comma before each
/// closing bracket: tsconfig's dialect, which must read back as the same value.
fn as_jsonc(v: &Value) -> String {
    let pretty = serde_json::to_string_pretty(v).unwrap();
    let lines: Vec<&str> = pretty.lines().collect();
    let mut out = String::from("/* head */\n");
    for (i, l) in lines.iter().enumerate() {
        out.push_str(l);
        let next = lines.get(i + 1).map(|n| n.trim_start());
        let closes = next.is_some_and(|n| n.starts_with('}') || n.starts_with(']'));
        if closes && !l.ends_with('{') && !l.ends_with('[') {
            out.push(',');
        }
        out.push_str(" // note, with \"quotes\" and /* */\n");
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let text = String::from_utf8_lossy(rest);
    match sel % 6 {
        0 => {
            let strict = serde_json::from_str::<Value>(&text).ok();
            let loose = parse_value(&text);
            // The JSONC reader stops at MAX_DEPTH levels; serde_json allows more.
            if let Some(v) = strict.as_ref().filter(|v| depth(v) <= MAX_DEPTH) {
                assert_eq!(loose.as_ref(), Some(v), "valid JSON read differently");
                // Against the pretty text's own value: floats need not round-trip.
                let pretty = serde_json::to_string_pretty(v).unwrap();
                let expect = serde_json::from_str::<Value>(&pretty).ok();
                assert_eq!(parse_value(&as_jsonc(v)), expect, "comments changed it");
            }
            if let Some(v) = loose {
                for e in entries(&v) {
                    assert!(
                        is_declaration_file(&e.types) && !e.types.contains('*'),
                        "{e:?}"
                    );
                    assert_eq!(normalize(&e.types).as_deref(), Some(e.types.as_str()));
                    if let Some(s) = &e.source {
                        assert!(!is_declaration_file(s), "{e:?}");
                        assert_eq!(normalize(s).as_deref(), Some(s.as_str()));
                    }
                }
            }
        }
        1 => {
            check_normal(&text);
            for part in text.split('\n') {
                check_normal(part);
            }
        }
        2 => {
            // A glob and a directory, split at the first newline. Globs come from the
            // repository's package.json, so matching must stay fast.
            let (glob, path) = text.split_once('\n').unwrap_or((&text, ""));
            let m = glob_match(glob, path);
            if !glob.contains('*') {
                fn segs(s: &str) -> Vec<&str> {
                    s.split('/').filter(|x| !x.is_empty()).collect()
                }
                assert_eq!(m, segs(glob) == segs(path), "{glob:?} {path:?}");
            }
            if glob.contains("**") && glob.split('/').all(|s| s.is_empty() || s == "**") {
                assert!(m, "{glob:?} should match {path:?}");
            }
        }
        3 => {
            for p in pnpm_packages(&text) {
                assert!(!p.is_empty());
            }
        }
        4 => {
            for r in reexports(&text) {
                if let Some(names) = &r.names {
                    for (local, exported) in names {
                        assert!(!local.is_empty() && !exported.is_empty(), "{r:?}");
                    }
                }
            }
        }
        _ => {
            if let Some(p) = parse_symbol(&text) {
                assert!(!p.name.is_empty() && !p.file.is_empty(), "{p:?}");
                assert!(text.ends_with(p.rest), "{p:?}");
            }
        }
    }
});
