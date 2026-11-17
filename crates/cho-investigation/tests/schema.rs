//! The JSON Schema and the Rust types describe the same format, and the parser's fuzz
//! property holds for every seed (a stand-in for a libFuzzer run where none is installed).
#![allow(clippy::disallowed_methods)] // tests read their own fixtures

use cho_investigation::Investigation;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn read(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap()
}

fn json_files(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    out.sort();
    out
}

/// Every property name the schema declares, anywhere.
fn schema_properties(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::Object(props)) = m.get("properties") {
                out.extend(props.keys().cloned());
            }
            m.values().for_each(|x| schema_properties(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| schema_properties(x, out)),
        _ => {}
    }
}

/// Every field name a file uses, skipping the keys of maps keyed by ids.
fn used_fields(v: &Value, out: &mut BTreeSet<String>) {
    const ID_MAPS: [&str; 3] = ["views", "cells", "rows"];
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                out.insert(k.clone());
                match x {
                    Value::Object(inner) if ID_MAPS.contains(&k.as_str()) => {
                        inner.values().for_each(|y| used_fields(y, out));
                    }
                    _ => used_fields(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| used_fields(x, out)),
        _ => {}
    }
}

#[test]
fn schema_declares_every_field_the_examples_use() {
    let schema: Value =
        serde_json::from_str(&read("../cho-cli/schema/investigation.schema.json")).unwrap();
    assert_eq!(
        schema["properties"]["chorong"]["enum"],
        serde_json::json!([1, 2])
    );
    let mut declared = BTreeSet::new();
    schema_properties(&schema, &mut declared);
    for path in json_files("../../docs/public/examples") {
        let file: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        // Round-trip through the Rust types too, so a renamed field shows up here.
        let inv = Investigation::parse(&file.to_string()).unwrap();
        let back: Value = serde_json::from_str(&inv.to_json()).unwrap();
        let mut used = BTreeSet::new();
        used_fields(&file, &mut used);
        used_fields(&back, &mut used);
        let missing: Vec<_> = used.difference(&declared).collect();
        assert!(missing.is_empty(), "{}: {missing:?}", path.display());
    }
}

/// The fuzz target's property: what parses re-parses from `to_json` to the same value.
fn property(s: &str) {
    if let Ok(inv) = Investigation::parse(s) {
        assert!(inv.validate().is_ok());
        let back = Investigation::parse(&inv.to_json())
            .unwrap_or_else(|e| panic!("to_json output failed to re-parse: {e}"));
        assert_eq!(back, inv);
    }
}

#[test]
fn fuzz_property_holds_for_seeds_and_simple_mutations() {
    let seeds = json_files("../../fuzz/seeds/investigation");
    assert!(seeds.len() >= 6, "the v2 examples are fuzz seeds");
    for path in seeds {
        let text = fs::read_to_string(&path).unwrap();
        property(&text);
        // Truncations and single-byte changes: never a panic.
        let bytes = text.as_bytes();
        for cut in (0..bytes.len()).step_by(7) {
            if let Ok(s) = std::str::from_utf8(&bytes[..cut]) {
                property(s);
            }
        }
        for at in (0..bytes.len()).step_by(11) {
            for b in *b"\"{12 " {
                let mut m = bytes.to_vec();
                m[at] = b;
                if let Ok(s) = std::str::from_utf8(&m) {
                    property(s);
                }
            }
        }
    }
}
