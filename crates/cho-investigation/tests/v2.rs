//! Format version 2: parsing, checking and the session API, against a small checkout.
#![allow(clippy::disallowed_methods)] // tests write and read their own fixtures

use cho_investigation::check::{
    CheckReport, EdgeOracle, Endpoint, NodeStatus, WorktreeReader, check, check_with_oracle,
};
use cho_investigation::check_v2::{PaneReport, ResultSource};
use cho_investigation::session::{Checker, InvestigationSession};
use cho_investigation::source::Source;
use cho_investigation::{FormatError, Investigation, Relation, RowResult, Verification};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

const SRC: &str = "\
fn route() {
    handle();
}
fn handle() {
    let ok = verify(sig);
    if !ok { return; }
    enqueue();
}
fn parse() {}
fn enqueue() {
    send(job);
}
fn other() {}
";

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

fn checkout(name: &str) -> Dir {
    let root = std::env::temp_dir().join(format!("cho-v2-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), SRC).unwrap();
    fs::write(root.join("src/other.rs"), "fn x() {}\n").unwrap();
    Dir(root)
}

fn node(id: &str, sym: &str) -> Value {
    json!({ "id": id, "anchor": { "path": "src/lib.rs", "symbol": sym } })
}

fn tour() -> Value {
    json!({
        "chorong": 2, "title": "t", "kind": "tour", "repo": "local:/x", "rev": "abcdef1",
        "nodes": [
            node("route", "route"),
            { "id": "handle", "name": "Verify", "evidence": "read", "values": ["ok = true"],
              "anchor": { "path": "src/lib.rs", "symbol": "handle", "from": "let ok", "to": "return; }" } },
            node("parse", "parse"),
            node("enqueue", "enqueue"),
            node("other", "other")
        ],
        "externals": [ { "id": "db", "what": "database", "label": "Jobs DB" } ],
        "edges": [
            { "from": "route", "to": "handle", "rel": "calls" },
            { "from": "handle", "to": "enqueue", "rel": "calls" },
            { "from": "enqueue", "to": "db", "rel": "calls" }
        ],
        "groups": [
            { "label": "Setup", "members": ["parse"], "phase": "setup" },
            { "label": "Call", "members": ["route", "handle", "enqueue"], "phase": "call" }
        ],
        "steps": ["parse", "route", "handle", "enqueue", "db"],
        "views": {
            "handle": { "panes": [
                { "kind": "editor", "highlights": [
                    { "anchor": "handle", "note": "first" },
                    { "anchor": { "path": "src/lib.rs", "symbol": "handle", "excerpt": "enqueue()" } }
                ] },
                { "kind": "graph", "of": "db" },
                { "kind": "context", "anchor": { "path": "src/lib.rs", "symbol": "nope" } }
            ] },
            "route": { "arrangement": "carousel", "panes": [ { "kind": "editor" } ] },
            "enqueue": { "panes": [ { "kind": "hologram", "x": 1 } ] }
        },
        "variants": [
            { "id": "early", "label": "Bad signature", "rows": {
                "enqueue": { "missing": "not reached: returns early" },
                "db": { "missing": "not reached" }
            } }
        ],
        "untraced": [ { "what": "plugin hooks", "at": { "path": "src/lib.rs", "symbol": "gone" } } ]
    })
}

fn parse(v: &Value) -> Result<Investigation, FormatError> {
    Investigation::parse(&v.to_string())
}

#[test]
fn version_one_rejects_version_two_fields() {
    let cases: Vec<(&str, Value)> = vec![
        ("points", json!({ "points": ["a"] })),
        (
            "externals",
            json!({ "externals": [ { "id": "x", "what": "model", "label": "M" } ] }),
        ),
        (
            "views",
            json!({ "views": { "a": { "panes": [ { "kind": "editor" } ] } } }),
        ),
        ("untraced", json!({ "untraced": [ { "what": "x" } ] })),
    ];
    for (name, extra) in cases {
        let mut v = json!({
            "chorong": 1, "title": "t", "kind": "tour", "repo": "local:/x", "rev": "abcdef1",
            "nodes": [ node("a", "route") ], "steps": ["a"]
        });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        let err = parse(&v).unwrap_err();
        assert!(
            matches!(&err, FormatError::Invalid(m) if m.contains("version 2")),
            "{name}: {err}"
        );
        v["chorong"] = 2.into();
        assert!(parse(&v).is_ok(), "{name} under v2");
    }
    // Node-level and anchor-level fields too.
    let mut v = json!({
        "chorong": 1, "title": "t", "kind": "tour", "repo": "local:/x", "rev": "abcdef1",
        "nodes": [ { "id": "a", "name": "n", "anchor": { "path": "src/lib.rs" } } ], "steps": ["a"]
    });
    assert!(parse(&v).is_err());
    v["nodes"][0] =
        json!({ "id": "a", "anchor": { "path": "src/lib.rs", "from": "a", "to": "b" } });
    assert!(parse(&v).is_err());
}

#[test]
fn version_one_report_has_no_version_two_keys() {
    let dir = checkout("v1json");
    let inv = parse(&json!({
        "chorong": 1, "title": "t", "kind": "callstack", "repo": "local:/x", "rev": "abcdef1",
        "nodes": [ node("a", "route"), node("b", "handle") ],
        "edges": [ { "from": "a", "to": "b", "rel": "calls" } ], "steps": ["a", "b"]
    }))
    .unwrap();
    let json = serde_json::to_value(check(&inv, &dir.0)).unwrap();
    let mut keys: Vec<&String> = json.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["edges", "nodes"]);
    assert!(json["nodes"][0].get("author_evidence").is_none());
}

#[test]
fn structural_errors_in_version_two() {
    type Mutation = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Mutation)> = vec![
        (
            "from without to",
            Box::new(|v| v["nodes"][1]["anchor"] = json!({ "path": "src/lib.rs", "from": "x" })),
        ),
        (
            "from with excerpt",
            Box::new(|v| v["nodes"][1]["anchor"]["excerpt"] = "x".into()),
        ),
        (
            "view on a non-step",
            Box::new(|v| v["views"]["other"] = json!({ "panes": [ { "kind": "editor" } ] })),
        ),
        (
            "empty panes",
            Box::new(|v| v["views"]["handle"]["panes"] = json!([])),
        ),
        (
            "editor on external",
            Box::new(|v| {
                v["views"]["handle"]["panes"][0] = json!({ "kind": "editor", "anchor": "db" })
            }),
        ),
        (
            "graph depth",
            Box::new(|v| v["views"]["handle"]["panes"][1]["depth"] = 9.into()),
        ),
        (
            "unknown field in known pane",
            Box::new(|v| v["views"]["handle"]["panes"][1]["size"] = 3.into()),
        ),
        (
            "duplicate external id",
            Box::new(|v| v["externals"][0]["id"] = "route".into()),
        ),
        (
            "branch on unknown step",
            Box::new(|v| v["variants"][0]["rows"]["zz"] = json!({ "missing": "x" })),
        ),
        (
            "branch cell with both",
            Box::new(|v| {
                v["variants"][0]["rows"]["db"] = json!({ "missing": "x", "step": "route" })
            }),
        ),
        (
            "branch result",
            Box::new(|v| v["variants"][0]["rows"]["db"]["result"] = "same".into()),
        ),
        (
            "blank value",
            Box::new(|v| v["nodes"][1]["values"] = json!([" "])),
        ),
        (
            "sides on a tour",
            Box::new(|v| v["sides"] = json!([ { "id": "a", "label": "A", "path": ["route"] } ])),
        ),
    ];
    assert!(parse(&tour()).is_ok());
    for (name, mutate) in cases {
        let mut v = tour();
        mutate(&mut v);
        assert!(parse(&v).is_err(), "{name}");
    }
}

#[test]
fn ranges_views_steps_and_externals() {
    let dir = checkout("tour");
    let inv = parse(&tour()).unwrap();
    let report = check(&inv, &dir.0);
    let v2 = report.v2.as_ref().unwrap();

    // `from`…`to` covers both ends.
    let handle = report.node("handle").unwrap();
    assert_eq!(handle.status, NodeStatus::Resolved);
    assert_eq!(
        &SRC[handle.range.clone().unwrap()],
        "let ok = verify(sig);\n    if !ok { return; }"
    );
    assert_eq!(
        handle.author_evidence,
        Some(cho_investigation::Evidence::Read)
    );

    // Numbers: setup S1, then 1, 2…; the external step is numbered too.
    let numbers: Vec<&str> = v2.steps.iter().map(|s| s.number.as_str()).collect();
    assert_eq!(numbers, ["S1", "1", "2", "3", "4"]);
    // Depth from the first step: `parse` calls nothing, so nothing has a depth but it.
    assert_eq!(v2.steps[0].depth.map(|d| d.n), Some(0));
    assert!(v2.steps[2].depth.is_none());
    // Called by, with Chorong's label.
    let callers = &v2.steps[2].called_by;
    assert_eq!(callers.len(), 1);
    assert_eq!(
        (callers[0].id.as_str(), callers[0].verification),
        ("route", Verification::Claimed)
    );
    assert!(v2.steps[4].external);
    assert_eq!(v2.externals[0].status, "not-in-code");
    // An edge to an external is claimed, never verified.
    assert_eq!(report.edges[2].status, "claimed");

    // The handle view: two highlights, a graph of an external, a broken context pane.
    let view = v2.views.iter().find(|v| v.step == "handle").unwrap();
    assert_eq!(view.arrangement, "main-side");
    let PaneReport::Editor {
        highlights, path, ..
    } = &view.panes[0]
    else {
        panic!("editor first")
    };
    assert_eq!(path.as_deref(), Some("src/lib.rs"));
    assert_eq!(highlights.len(), 2);
    // A file without `name`: the lone note is the label, with no commentary.
    assert_eq!(highlights[0].name.as_deref(), Some("first"));
    assert_eq!(highlights[0].note, None);
    assert_eq!(
        &SRC[highlights[1].anchor.range.clone().unwrap()],
        "enqueue()"
    );
    assert!(matches!(
        view.panes[1],
        PaneReport::Graph { external: true, .. }
    ));
    let PaneReport::Context { anchor } = &view.panes[2] else {
        panic!()
    };
    assert_eq!(anchor.status, NodeStatus::Broken);

    // Unknown arrangement and unknown pane kind: reported, default page.
    for step in ["route", "enqueue"] {
        let v = v2.views.iter().find(|v| v.step == step).unwrap();
        assert!(v.default && v.fallback.is_some(), "{step}");
        assert_eq!(v.panes.len(), 2);
    }
    assert!(v2.messages.iter().any(|m| m.contains("carousel")));
    assert!(v2.messages.iter().any(|m| m.contains("hologram")));

    // Broken: the context pane anchor and the untraced `at`; not the fallen-back views.
    assert_eq!(report.broken_count(), 0);
    assert_eq!(report.broken_total(), 2);

    // Branches report only what they change.
    assert_eq!(v2.branches[0].overrides.len(), 2);
    assert!(!v2.branches[0].overrides[0].reached);

    // Unknown kinds round-trip.
    let back = Investigation::parse(&inv.to_json()).unwrap();
    assert_eq!(back, inv);
}

#[test]
fn missing_range_end_breaks_the_anchor() {
    let dir = checkout("range");
    let mut v = tour();
    v["nodes"][1]["anchor"]["to"] = "never there".into();
    let report = check(&parse(&v).unwrap(), &dir.0);
    let n = report.node("handle").unwrap();
    assert_eq!(n.status, NodeStatus::Broken);
    assert!(n.messages[0].contains("`to`"));
    v["nodes"][1]["anchor"]["from"] = "nor this".into();
    let report = check(&parse(&v).unwrap(), &dir.0);
    assert!(report.node("handle").unwrap().messages[0].contains("`from`"));
}

struct Yes;
impl EdgeOracle for Yes {
    fn confirms(&self, _: Relation, _: &Endpoint<'_>, _: &Endpoint<'_>) -> bool {
        true
    }
}

#[test]
fn depth_is_verified_only_along_verified_edges() {
    let dir = checkout("depth");
    let mut v = tour();
    v["steps"] = json!(["route", "handle", "enqueue", "db"]);
    v["groups"] = json!([]);
    v["variants"] = json!([]);
    v["views"] = json!({});
    let inv = parse(&v).unwrap();
    let r = check_with_oracle(&inv, &dir.0, Some(&Yes));
    let steps = &r.v2.as_ref().unwrap().steps;
    let depth = |i: usize| steps[i].depth.map(|d| (d.n, d.verification));
    assert_eq!(depth(1), Some((1, Verification::Verified)));
    assert_eq!(depth(2), Some((2, Verification::Verified)));
    // The edge to the external is never verified.
    assert_eq!(depth(3), Some((3, Verification::Claimed)));
    let numbers: Vec<&str> = steps.iter().map(|s| s.number.as_str()).collect();
    assert_eq!(numbers, ["1", "2", "3", "4"]);
}

fn comparison() -> Value {
    json!({
        "chorong": 2, "title": "c", "kind": "comparison", "repo": "local:/x", "rev": "abcdef1",
        "nodes": [ node("route", "route"), node("handle", "handle"), node("parse", "parse"),
                   node("enqueue", "enqueue"), node("other", "other") ],
        "externals": [ { "id": "db", "what": "database", "label": "DB" } ],
        "sides": [
            { "id": "a", "label": "A", "path": ["route", "handle", "enqueue"],
              "variants": [ { "id": "alt", "label": "Alt", "rows": {
                  "work": { "step": "enqueue" },
                  "start": { "missing": "no start in alt", "callout": "alt starts late" } } } ] },
            { "id": "b", "label": "B", "path": ["parse", "enqueue", "other"] }
        ],
        "rows": [
            { "id": "setup", "name": "Setup", "phase": "setup",
              "cells": { "a": { "missing": "none" }, "b": { "step": "parse" } } },
            { "id": "start", "name": "Start", "phase": "call",
              "cells": { "a": { "step": "route" }, "b": { "step": "other" } }, "result": "differs" },
            { "id": "work", "name": "Work", "phase": "call",
              "cells": { "a": { "step": "handle" }, "b": { "step": "enqueue" } } },
            { "id": "end", "name": "End", "phase": "call",
              "cells": { "a": { "step": "enqueue" }, "b": { "step": "enqueue" } }, "result": "differs" }
        ]
    })
}

#[test]
fn comparison_rows_results_and_variants() {
    let dir = checkout("cmp");
    let inv = parse(&comparison()).unwrap();
    let report = check(&inv, &dir.0);
    let cmp = report.v2.as_ref().unwrap().comparison.as_ref().unwrap();
    let r = |id: &str| cmp.rows.iter().find(|r| r.id == id).unwrap();
    assert_eq!(r("setup").number, "S1");
    assert_eq!(r("start").number, "1");
    assert_eq!(
        (r("setup").result, r("setup").result_source),
        (Some(RowResult::Missing), ResultSource::Derived)
    );
    assert_eq!(
        (r("start").result, r("start").result_source),
        (Some(RowResult::Differs), ResultSource::Claimed)
    );
    // No result written and different code: reported, no result.
    assert_eq!(r("work").result_source, ResultSource::None);
    // Same declaration on both sides: a fact, but the written `differs` stands.
    assert_eq!(
        (r("end").result, r("end").result_source, r("end").same_code),
        (Some(RowResult::Differs), ResultSource::Claimed, true)
    );
    assert_eq!(r("end").messages, ["same code; differs by values"]);
    assert!(!r("work").same_code);
    // `other` comes after `enqueue` on B's path, but row `start` puts it first.
    assert!(
        r("work")
            .messages
            .iter()
            .any(|m| m.contains("comes before"))
    );
    // Cells resolve per side.
    assert_eq!(r("work").cells[1].anchor.as_ref().unwrap().line, Some(10));

    // The variant's rows, resolved with it chosen.
    let alt = &cmp.variants[0];
    assert_eq!(alt.rows.len(), 2);
    let work = alt.rows.iter().find(|r| r.id == "work").unwrap();
    // Same code, nothing written, no values: derived `same`.
    assert_eq!(
        (work.result, work.result_source),
        (Some(RowResult::Same), ResultSource::Derived)
    );
    assert_eq!(work.cells[0].variant.as_deref(), Some("alt"));
    let start = alt.rows.iter().find(|r| r.id == "start").unwrap();
    assert_eq!(start.callout.as_deref(), Some("alt starts late"));
    assert_eq!(start.result, Some(RowResult::Missing));
}

fn write_session(dir: &Dir, v: &Value) -> InvestigationSession {
    let file = dir.0.join("inv.json");
    fs::write(&file, v.to_string()).unwrap();
    let reader = WorktreeReader { root: &dir.0 };
    InvestigationSession::open(
        Source::Path(file),
        Some(&dir.0),
        &Checker {
            reader: &reader,
            oracle: None,
        },
    )
    .unwrap()
}

#[test]
fn session_pages_branches_and_variants() {
    let dir = checkout("session");
    let mut s = write_session(&dir, &tour());
    // Step pages: written view, default page, fallback.
    let page = s.step_page(2).unwrap();
    assert!(!page.default && page.panes.len() == 3);
    let page = s.step_page(0).unwrap();
    assert!(page.default && page.panes.len() == 2);
    // The external step: no code, so no panes, and its own label.
    let ext = s.step(4).unwrap();
    assert!(ext.external.is_some() && ext.anchor.is_none() && ext.status.is_none());
    assert_eq!(ext.title, "Jobs DB");
    assert!(s.step_page(4).unwrap().panes.is_empty());
    let handle = s.step(2).unwrap();
    assert_eq!((handle.number.as_str(), handle.name), ("2", Some("Verify")));
    assert_eq!(handle.values, ["ok = true"]);

    // A branch: steps off the path are not reached.
    assert!(!s.set_branch(Some("nope")));
    assert!(s.set_branch(Some("early")));
    let enq = s.step(3).unwrap();
    assert!(!enq.reached);
    assert_eq!(enq.not_reached, Some("not reached: returns early"));
    assert!(s.step_page(3).unwrap().panes.is_empty());
    assert_eq!(s.branch_steps().iter().filter(|b| b.reached).count(), 3);
    assert!(s.set_branch(None));
    assert!(s.step(3).unwrap().reached);

    // Comparisons: choosing a variant changes the rows.
    let mut c = write_session(&dir, &comparison());
    assert_eq!(c.comparison_rows()[2].result_source, ResultSource::None);
    assert!(!c.set_variant("a", Some("zz")) && !c.set_variant("zz", None));
    assert!(c.set_variant("a", Some("alt")));
    assert_eq!(c.variant("a"), Some("alt"));
    assert_eq!(c.comparison_rows()[2].result, Some(RowResult::Same));
    assert!(c.set_variant("a", None));
    assert_eq!(c.comparison_rows()[2].result, None);
}

#[test]
fn documented_v2_examples_parse_and_report() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/public/examples");
    for name in [
        "tour-views.json",
        "comparison-rest-soap.json",
        "callstack-external.json",
        "tour-actions.json",
    ] {
        let json = fs::read_to_string(dir.join(name)).unwrap();
        let inv = Investigation::parse(&json).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(inv.cho, 2, "{name}");
        // Checked against an empty checkout, every anchor is broken but nothing panics and
        // the version 2 sections are there.
        let empty = checkout(&format!("ex-{}", name.len()));
        let report: CheckReport = check(&inv, &empty.0);
        assert!(report.v2.is_some(), "{name}");
        assert!(serde_json::to_string(&report).is_ok());
    }
}

#[test]
fn same_code_with_values_needs_a_written_result() {
    let dir = checkout("values");
    let mut v = comparison();
    v["rows"][3]["cells"]["a"]["values"] = json!(["flag = true"]);
    v["rows"][3]["cells"]["b"]["values"] = json!(["flag = false"]);
    v["rows"][3].as_object_mut().unwrap().remove("result");
    let report = check(&parse(&v).unwrap(), &dir.0);
    let row = &report
        .v2
        .as_ref()
        .unwrap()
        .comparison
        .as_ref()
        .unwrap()
        .rows[3];
    assert!(row.same_code);
    assert_eq!((row.result, row.result_source), (None, ResultSource::None));
    // A missing cell always wins, even over a written result.
    v["rows"][3]["cells"]["b"] = json!({ "missing": "not on B" });
    v["rows"][3]["result"] = "same".into();
    let report = check(&parse(&v).unwrap(), &dir.0);
    let row = &report
        .v2
        .as_ref()
        .unwrap()
        .comparison
        .as_ref()
        .unwrap()
        .rows[3];
    assert_eq!(
        (row.result, row.result_source),
        (Some(RowResult::Missing), ResultSource::Derived)
    );
}

#[test]
fn a_tour_whose_second_step_is_external() {
    let dir = checkout("ext2");
    let v = json!({
        "chorong": 2, "title": "t", "kind": "tour", "repo": "local:/x", "rev": "abcdef1",
        "nodes": [ node("route", "route"), node("handle", "handle") ],
        "externals": [ { "id": "vendor", "what": "vendor", "label": "Payment vendor", "values": ["refused"] } ],
        "edges": [ { "from": "route", "to": "vendor", "rel": "calls" },
                   { "from": "vendor", "to": "handle", "rel": "dispatches" } ],
        "steps": ["route", "vendor", "handle"],
        "views": { "handle": { "panes": [ { "kind": "graph", "of": "vendor", "direction": "in" } ] } }
    });
    let mut s = write_session(&dir, &v);
    assert_eq!(s.steps().len(), 3);
    assert!(s.next_step());
    let step = s.current_step().unwrap();
    assert_eq!(step.id, "vendor");
    assert!(step.external.is_some() && step.node.is_none() && step.anchor.is_none());
    assert_eq!(step.status, None);
    assert_eq!(step.values, ["refused"]);
    assert_eq!(step.title, "Payment vendor");
    assert!(step.messages.is_empty());
    assert_eq!(step.called_by.len(), 1);
    assert!(s.step_page(1).unwrap().panes.is_empty());
    assert_eq!(s.edges_into_step(1).len(), 1);
    assert_eq!(s.edges_into_step(2)[0].verification, Verification::Claimed);
    assert!(s.go_to_step("vendor") && s.go_to_step("handle"));
    assert_eq!(s.step(2).unwrap().depth.map(|d| d.n), Some(2));
    let page = s.step_page(2).unwrap();
    assert!(matches!(
        page.panes[0],
        PaneReport::Graph { external: true, .. }
    ));
    let v2 = s.v2().unwrap();
    assert_eq!(v2.externals[0].status, "not-in-code");
    assert!(serde_json::to_string(s.report()).is_ok());
}
