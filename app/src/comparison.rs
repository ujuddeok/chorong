//! Comparisons, variants and tour branches in the viewer, without GPUI (format version 2;
//! design/investigations-viewer.md, "Format version 2 in the viewer (decided)").
//!
//! A comparison's rows become steps ([`row_steps`]), so the Comparison pane, the focus page a
//! row opens and the comparison opened as a tour all use the tour machinery: one page per row,
//! one editor per side. Each row keeps what the pane draws in [`RowInfo`]: the result (by a
//! glyph as well as colour), where it comes from (derived by Chorong or the author's claim),
//! `same_code`, the callout and each side's cell.
//!
//! The rows themselves are resolved by `cho_investigation::check_v2::resolve_rows` with the
//! chosen variants, exactly as `InvestigationSession::comparison_rows` does: Chorong never
//! upgrades a written claim. The choices (a tour's branch, each side's variant) are remembered
//! per investigation in Chorong's config directory ([`load_choice`]), like the reader's place.

use crate::investigation::{Label, Step, file_name};
use crate::tour_pages::{Arrangement, PaneSpec, StepExtras, View};
use cho_investigation::Investigation;
use cho_investigation::check::NodeStatus;
use cho_investigation::check_v2::{CellReport, ResultSource, RowReport};
use cho_investigation::compare::RowResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A row's result as shown: each has its own glyph, so it reads without colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Same,
    Differs,
    Missing,
    /// Not written and not derivable (check reports it).
    Unset,
}

impl Outcome {
    pub fn of(r: Option<RowResult>) -> Outcome {
        match r {
            Some(RowResult::Same) => Outcome::Same,
            Some(RowResult::Differs) => Outcome::Differs,
            Some(RowResult::Missing) => Outcome::Missing,
            None => Outcome::Unset,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Outcome::Same => "same",
            Outcome::Differs => "differs",
            Outcome::Missing => "missing",
            Outcome::Unset => "no result",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Outcome::Same => "=",
            Outcome::Differs => "\u{2260}",
            Outcome::Missing => "\u{2205}",
            Outcome::Unset => "?",
        }
    }
}

/// Where a row's result comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Chorong worked it out (a side is missing, or the same code with nothing written).
    Derived,
    /// The author's claim.
    Claimed,
    None,
}

impl Source {
    fn of(s: &ResultSource) -> Source {
        match s {
            ResultSource::Derived => Source::Derived,
            ResultSource::Claimed => Source::Claimed,
            ResultSource::None => Source::None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Source::Derived => "derived",
            Source::Claimed => "claimed",
            Source::None => "",
        }
    }
}

/// One side's cell in a row, as the pane and the note strips show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideCell {
    /// `A`, `B`…, by side order.
    pub letter: String,
    pub side: String,
    pub side_label: String,
    /// The variant this cell comes from, by label; `None` for the side's base path.
    pub variant: Option<String>,
    /// The step's short name, else its label or declaration; `None` when missing.
    pub text: Option<String>,
    /// `orders.rs · create_order`, for code.
    pub location: Option<String>,
    /// Why this side has no step for the row.
    pub missing: Option<String>,
    /// For a node that is not in the code: what it is (`a database`).
    pub external: Option<String>,
    /// Why the step's anchor did not resolve.
    pub broken: Option<String>,
    /// Runtime values, as the author saw them (claims).
    pub values: Vec<String>,
    pub note: Option<String>,
}

impl SideCell {
    /// `A · REST handler`, with the variant when one is chosen.
    pub fn heading(&self) -> String {
        match &self.variant {
            Some(v) => format!("{} \u{b7} {} ({v})", self.letter, self.side_label),
            None => format!("{} \u{b7} {}", self.letter, self.side_label),
        }
    }
}

/// What a comparison row shows besides its name and number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowInfo {
    pub outcome: Outcome,
    pub source: Source,
    /// Every side anchors the same declaration (a fact; the result may still be "differs").
    pub same_code: bool,
    pub callout: Option<String>,
    /// One per side, in side order.
    pub cells: Vec<SideCell>,
    /// For each editor pane on the row's page, in order, the cell it shows.
    pub editor_cells: Vec<usize>,
    /// What check says about the row (`same code; differs by values`).
    pub messages: Vec<String>,
}

impl RowInfo {
    /// The result chip's words: `≠ differs · claimed`, `= same · derived · same code`.
    pub fn chip(&self) -> String {
        let mut t = format!("{} {}", self.outcome.glyph(), self.outcome.word());
        if self.source != Source::None {
            t.push_str(" \u{b7} ");
            t.push_str(self.source.word());
        }
        if self.same_code {
            t.push_str(" \u{b7} same code");
        }
        t
    }

    /// The first missing side's reason, for the chip's tooltip and the tour's note strip.
    pub fn missing_reason(&self) -> Option<String> {
        self.cells
            .iter()
            .find_map(|c| c.missing.as_ref().map(|m| format!("{}: {m}", c.heading())))
    }
}

/// `A`, `B`… `Z`, then `S27`…
pub fn letter(i: usize) -> String {
    match u8::try_from(i).ok().filter(|i| *i < 26) {
        Some(i) => char::from(b'A' + i).to_string(),
        None => format!("S{}", i + 1),
    }
}

/// How many rows have each result: (differs, missing, same, no result).
pub fn counts(steps: &[Step]) -> (usize, usize, usize, usize) {
    let n = |o| {
        steps
            .iter()
            .filter(|s| s.extras.row.as_ref().is_some_and(|r| r.outcome == o))
            .count()
    };
    (
        n(Outcome::Differs),
        n(Outcome::Missing),
        n(Outcome::Same),
        n(Outcome::Unset),
    )
}

fn cell_of(inv: &Investigation, i: usize, c: &CellReport) -> SideCell {
    let side = inv.sides.iter().find(|s| s.id == c.side);
    let variant = c.variant.as_ref().and_then(|v| {
        side.and_then(|s| s.variants.iter().find(|x| &x.id == v))
            .map(|x| x.label.clone())
    });
    let mut cell = SideCell {
        letter: letter(i),
        side: c.side.clone(),
        side_label: side.map_or_else(|| c.side.clone(), |s| s.label.clone()),
        variant,
        text: None,
        location: None,
        missing: c.missing.clone(),
        external: None,
        broken: None,
        values: c.values.clone(),
        note: c.note.clone(),
    };
    let Some(id) = &c.step else {
        if cell.missing.is_none() {
            cell.missing = Some("no step for this row".into());
        }
        return cell;
    };
    match inv.lookup(id) {
        Some(Ok(node)) => {
            cell.text = Some(
                node.name
                    .clone()
                    .unwrap_or_else(|| cho_investigation::session::node_title(node)),
            );
            if cell.note.is_none() {
                cell.note = node.note.clone();
            }
        }
        Some(Err(ext)) => {
            cell.text = Some(ext.name.clone().unwrap_or_else(|| ext.label.clone()));
            cell.external = Some(crate::investigation::external_word(&ext.what).to_string());
            if cell.note.is_none() {
                cell.note = ext.note.clone();
            }
        }
        None => cell.text = Some(id.clone()),
    }
    if let Some(a) = &c.anchor {
        cell.location = Some(match &a.label {
            l if l.as_str() == file_name(&a.path) => match a.line {
                Some(n) => format!("{}:{n}", file_name(&a.path)),
                None => l.clone(),
            },
            l => format!("{} \u{b7} {l}", file_name(&a.path)),
        });
        if a.status == NodeStatus::Broken {
            cell.broken = Some(
                a.messages
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "anchor not found".into()),
            );
        }
    }
    cell
}

/// The comparison's rows as steps, in file order: the row's name and number (S1… for setup
/// rows), and a page with one pane per side, side by side: an editor at the side's anchor
/// (tinted), or a card for a missing side, a node not in the code or a broken anchor.
pub fn row_steps(inv: &Investigation, rows: &[RowReport]) -> Vec<Step> {
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let cells: Vec<SideCell> = row
                .cells
                .iter()
                .enumerate()
                .map(|(k, c)| cell_of(inv, k, c))
                .collect();
            let mut panes = Vec::new();
            let mut editor_cells = Vec::new();
            let mut first: Option<&cho_investigation::check_v2::AnchorReport> = None;
            for (k, (c, cell)) in row.cells.iter().zip(&cells).enumerate() {
                let spec = match (&c.anchor, &cell.missing, &cell.external) {
                    (_, Some(reason), _) => PaneSpec::Missing {
                        side: cell.heading(),
                        reason: reason.clone(),
                    },
                    (_, None, Some(_)) => PaneSpec::External {
                        title: format!(
                            "{}: {}",
                            cell.heading(),
                            cell.text.clone().unwrap_or_default()
                        ),
                        note: cell.note.clone(),
                    },
                    (Some(a), None, None) if a.status != NodeStatus::Broken => {
                        first.get_or_insert(a);
                        editor_cells.push(k);
                        PaneSpec::Editor {
                            path: a.path.clone(),
                            caret: a.caret(),
                            region: a.region.clone(),
                            highlights: Vec::new(),
                            at: a.label.clone(),
                        }
                    }
                    (Some(a), None, None) => PaneSpec::Broken {
                        asked: format!("{} in {}", a.label, a.path),
                        reason: cell.broken.clone().unwrap_or_default(),
                    },
                    (None, None, None) => PaneSpec::Missing {
                        side: cell.heading(),
                        reason: "no step for this row".into(),
                    },
                };
                panes.push(spec);
            }
            let broken = cells.iter().any(|c| c.broken.is_some());
            let all_code = cells
                .iter()
                .all(|c| c.missing.is_none() && c.external.is_none() && c.broken.is_none());
            let info = RowInfo {
                outcome: Outcome::of(row.result),
                source: Source::of(&row.result_source),
                same_code: row.same_code,
                callout: row.callout.clone(),
                cells,
                editor_cells,
                messages: row.messages.clone(),
            };
            let arrangement = if panes.len() == 4 {
                Arrangement::Grid
            } else {
                Arrangement::SideBySide
            };
            Step {
                id: row.id.clone(),
                n: i + 1,
                title: row.name.clone(),
                symbol: None,
                path: first.map(|a| a.path.clone()).unwrap_or_default(),
                label: if broken {
                    Label::Broken
                } else if all_code {
                    Label::Verified
                } else {
                    Label::Claimed
                },
                reason: None,
                range: first.and_then(|a| a.range.clone()),
                region: first.and_then(|a| a.region.clone()),
                name_start: first.and_then(|a| a.name_range.as_ref().map(|r| r.start)),
                line: first.and_then(|a| a.line),
                note: row.note.clone(),
                number: row.number.clone(),
                extras: StepExtras {
                    name: None,
                    phase: row.phase,
                    view: Some(View { panes, arrangement }),
                    row: Some(Arc::new(info)),
                    ..Default::default()
                },
            }
        })
        .collect()
}

// ---- Remembered choices ----

/// What the reader chose in one investigation: the tour branch and each side's variant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Side id to variant id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, String>,
    /// When it was last changed (seconds), so the oldest are dropped first.
    #[serde(default)]
    pub at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ChoicesFile {
    /// By investigation (its source key, as recents are).
    #[serde(default)]
    by_source: BTreeMap<String, Choice>,
}

const CHOICES_FILE: &str = "investigation-choices.json";
/// Investigations whose choices are kept.
const KEEP: usize = 100;

/// The choices remembered for `key` (empty when none, or no config directory).
pub fn load_choice(key: &str) -> Choice {
    crate::prefs::file(CHOICES_FILE)
        .map(|p| crate::prefs::load::<ChoicesFile>(&p))
        .and_then(|mut f| f.by_source.remove(key))
        .unwrap_or_default()
}

/// Remembers `choice` for `key` (forgetting it when empty), keeping the newest [`KEEP`].
pub fn save_choice(key: &str, choice: &Choice) -> Result<(), String> {
    let Some(path) = crate::prefs::file(CHOICES_FILE) else {
        return Ok(());
    };
    let mut file: ChoicesFile = crate::prefs::load(&path);
    put_choice(&mut file.by_source, key, choice);
    crate::prefs::save_named(CHOICES_FILE, &file)
}

fn put_choice(map: &mut BTreeMap<String, Choice>, key: &str, choice: &Choice) {
    if choice.branch.is_none() && choice.variants.is_empty() {
        map.remove(key);
    } else {
        map.insert(key.to_string(), choice.clone());
    }
    while map.len() > KEEP {
        let Some(oldest) = map.iter().min_by_key(|(_, c)| c.at).map(|(k, _)| k.clone()) else {
            break;
        };
        map.remove(&oldest);
    }
}

/// `choice` with only what `inv` still has: a branch or variant id the file no longer names
/// is dropped (a reload may have removed it).
pub fn valid_choice(inv: &Investigation, mut choice: Choice) -> Choice {
    if choice
        .branch
        .as_ref()
        .is_some_and(|b| !inv.variants.iter().any(|v| &v.id == b))
    {
        choice.branch = None;
    }
    choice.variants.retain(|side, v| {
        inv.sides
            .iter()
            .any(|s| &s.id == side && s.variants.iter().any(|x| &x.id == v))
    });
    choice
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::investigation::CodeAt;
    use cho_investigation::check::SourceReader;

    struct Files(Vec<(&'static str, &'static str)>);

    impl SourceReader for Files {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            self.0
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.as_bytes().to_vec())
                .ok_or_else(|| format!("{path}: no such file"))
        }
    }

    fn files() -> Files {
        Files(vec![
            (
                "src/rest.rs",
                "pub fn routes() {}\n\npub fn create(order: u32) -> u32 {\n    let checked = validate(order);\n    checked\n}\n\npub fn create_bulk(n: u32) -> u32 {\n    n\n}\n\nfn validate(o: u32) -> u32 {\n    o\n}\n",
            ),
            (
                "src/soap.rs",
                "pub fn dispatch(body: u32) -> u32 {\n    crate::wrap::call(body)\n}\n",
            ),
            ("src/wrap.rs", "pub fn call(x: u32) -> u32 {\n    x\n}\n"),
        ])
    }

    fn comparison() -> Investigation {
        serde_json::from_value(serde_json::json!({
            "chorong": 2, "title": "REST against SOAP", "kind": "comparison",
            "repo": "local:/x", "rev": "abc",
            "nodes": [
                { "id": "route", "anchor": { "path": "src/rest.rs", "symbol": "routes" } },
                { "id": "rest", "name": "Create", "anchor": { "path": "src/rest.rs", "symbol": "create" } },
                { "id": "bulk", "anchor": { "path": "src/rest.rs", "symbol": "create_bulk" } },
                { "id": "soap", "anchor": { "path": "src/soap.rs", "symbol": "dispatch" } },
                { "id": "wrap", "anchor": { "path": "src/wrap.rs", "symbol": "call" } },
                { "id": "gone", "anchor": { "path": "src/soap.rs", "symbol": "nowhere" } }
            ],
            "externals": [ { "id": "db", "what": "database", "label": "Orders database" } ],
            "sides": [
                { "id": "rest", "label": "REST", "path": ["route", "rest", "wrap", "db"],
                  "variants": [ { "id": "bulk", "label": "Bulk", "rows": {
                      "entry": { "step": "bulk" },
                      "govern": { "missing": "bulk skips the wrapper" } } } ] },
                { "id": "soap", "label": "SOAP", "path": ["soap", "wrap", "db"] }
            ],
            "rows": [
                { "id": "reg", "name": "Register", "phase": "setup",
                  "cells": { "rest": { "step": "route" }, "soap": { "missing": "deployment config" } } },
                { "id": "entry", "name": "Receive", "phase": "call",
                  "cells": { "rest": { "step": "rest" }, "soap": { "step": "soap" } },
                  "result": "differs", "callout": "Paths split here" },
                { "id": "govern", "name": "Governed call", "phase": "call",
                  "cells": { "rest": { "step": "wrap" }, "soap": { "step": "wrap" } } },
                { "id": "store", "name": "Store", "phase": "call",
                  "cells": { "rest": { "step": "db", "values": ["rows = 1"] }, "soap": { "step": "gone" } },
                  "result": "differs" }
            ]
        }))
        .unwrap()
    }

    fn rows(inv: &Investigation, choices: &BTreeMap<String, String>) -> Vec<RowReport> {
        let report = cho_investigation::check::check_with_reader(inv, &files(), None);
        cho_investigation::check_v2::resolve_rows(inv, &report, choices)
    }

    #[test]
    fn rows_become_steps_with_one_pane_per_side() {
        let inv = comparison();
        let steps = row_steps(&inv, &rows(&inv, &BTreeMap::new()));
        let numbers: Vec<_> = steps.iter().map(|s| s.number.as_str()).collect();
        assert_eq!(numbers, ["S1", "1", "2", "3"]);
        let reg = steps[0].extras.row.as_ref().unwrap();
        // A missing side wins, derived; the page has the editor and a card that says why.
        assert_eq!(
            (reg.outcome, reg.source),
            (Outcome::Missing, Source::Derived)
        );
        let view = steps[0].extras.view.as_ref().unwrap();
        assert!(matches!(&view.panes[0], PaneSpec::Editor { path, .. } if path == "src/rest.rs"));
        assert!(
            matches!(&view.panes[1], PaneSpec::Missing { side, reason } if side == "B \u{b7} SOAP" && reason == "deployment config")
        );
        assert_eq!(reg.editor_cells, [0]);
        // A written "differs" stands, as the author's claim, with its callout.
        let entry = steps[1].extras.row.as_ref().unwrap();
        assert_eq!(entry.chip(), "\u{2260} differs \u{b7} claimed");
        assert_eq!(entry.callout.as_deref(), Some("Paths split here"));
        assert_eq!(entry.cells[0].text.as_deref(), Some("Create"));
        assert_eq!(
            entry.cells[1].location.as_deref(),
            Some("soap.rs \u{b7} dispatch")
        );
        assert_eq!(entry.editor_cells, [0, 1]);
        // The same declaration with nothing written: same, derived, same code.
        let govern = steps[2].extras.row.as_ref().unwrap();
        assert_eq!(govern.chip(), "= same \u{b7} derived \u{b7} same code");
        // An external node and a broken anchor are cards, never editors.
        let store = steps[3].extras.row.as_ref().unwrap();
        assert_eq!(store.cells[0].external.as_deref(), Some("a database"));
        assert_eq!(store.cells[0].values, ["rows = 1"]);
        assert!(store.cells[1].broken.is_some());
        assert_eq!(steps[3].label, Label::Broken);
        assert!(store.editor_cells.is_empty());
        assert_eq!(counts(&steps), (2, 1, 1, 0));
    }

    #[test]
    fn a_variant_changes_only_the_rows_it_names() {
        let inv = comparison();
        let base = row_steps(&inv, &rows(&inv, &BTreeMap::new()));
        let chosen = BTreeMap::from([("rest".to_string(), "bulk".to_string())]);
        let bulk = row_steps(&inv, &rows(&inv, &chosen));
        let entry = bulk[1].extras.row.as_ref().unwrap();
        assert_eq!(entry.cells[0].variant.as_deref(), Some("Bulk"));
        assert_eq!(entry.cells[0].heading(), "A \u{b7} REST (Bulk)");
        assert_eq!(
            entry.cells[0].location.as_deref(),
            Some("rest.rs \u{b7} create_bulk")
        );
        // The variant's missing cell makes that row missing, derived.
        let govern = bulk[2].extras.row.as_ref().unwrap();
        assert_eq!(govern.outcome, Outcome::Missing);
        assert_eq!(
            govern.missing_reason().as_deref(),
            Some("A \u{b7} REST (Bulk): bulk skips the wrapper")
        );
        // Rows it does not name are unchanged.
        assert_eq!(base[0], bulk[0]);
        assert_eq!(base[3], bulk[3]);
    }

    #[test]
    fn choices_keep_only_what_the_file_still_names_and_the_newest() {
        let inv = comparison();
        let c = Choice {
            branch: Some("nope".into()),
            variants: BTreeMap::from([
                ("rest".into(), "bulk".into()),
                ("soap".into(), "bulk".into()),
            ]),
            at: 1,
        };
        let v = valid_choice(&inv, c);
        assert_eq!(v.branch, None);
        assert_eq!(v.variants.len(), 1);
        let mut map = BTreeMap::new();
        for i in 0..(KEEP as u64 + 5) {
            put_choice(
                &mut map,
                &format!("k{i}"),
                &Choice {
                    branch: Some("b".into()),
                    at: i,
                    ..Default::default()
                },
            );
        }
        assert_eq!(map.len(), KEEP);
        assert!(!map.contains_key("k0") && map.contains_key(&format!("k{}", KEEP + 4)));
        put_choice(&mut map, "k50", &Choice::default());
        assert!(!map.contains_key("k50"));
    }

    fn tour() -> Investigation {
        serde_json::from_value(serde_json::json!({
            "chorong": 2, "title": "t", "kind": "tour", "repo": "local:/x", "rev": "abc",
            "nodes": [
                { "id": "a", "name": "Routes", "anchor": { "path": "src/rest.rs", "symbol": "routes" } },
                { "id": "b", "name": "Create", "anchor": { "path": "src/rest.rs", "symbol": "create" } },
                { "id": "b2", "anchor": { "path": "src/rest.rs", "symbol": "create_bulk" } },
                { "id": "c", "anchor": { "path": "src/wrap.rs", "symbol": "call" } }
            ],
            "steps": ["a", "b", "c"],
            "variants": [ { "id": "early", "label": "Stops early", "rows": {
                "b": { "step": "b2", "values": ["n = 3"] },
                "c": { "missing": "not reached: returns first" } } } ]
        }))
        .unwrap()
    }

    #[test]
    fn a_branch_marks_steps_not_reached_and_stepping_skips_them() {
        let inv = tour();
        let report = cho_investigation::check::check_with_reader(&inv, &files(), None);
        let at = CodeAt::WorkingTree;
        let branch = inv.variants.first();
        let (extras, _) = crate::investigation::v2_of(&inv, &report);
        let steps = crate::investigation::apply_branch(
            crate::investigation::with_extras(
                crate::investigation::steps_on(&inv, &report, &at, branch),
                &extras,
            ),
            &inv,
            branch,
        );
        assert_eq!(steps.len(), 3, "a step not reached is never dropped");
        // Replaced: the slot keeps its id, shows the other node, with the branch's values.
        assert_eq!(steps[1].id, "b");
        assert_eq!(steps[1].symbol.as_deref(), Some("create_bulk"));
        assert_eq!(steps[1].extras.name, None);
        assert_eq!(steps[1].extras.runtime, ["n = 3"]);
        assert_eq!(
            steps[2].extras.not_reached.as_deref(),
            Some("not reached: returns first")
        );
        assert_eq!(crate::investigation::step_to_reached(&steps, 1, 1), 1);
        assert_eq!(crate::investigation::step_to_reached(&steps, 0, 2), 1);
        assert_eq!(crate::investigation::step_to_reached(&steps, 2, -1), 1);
        assert_eq!(crate::investigation::reached_ends(&steps), Some((0, 1)));
        // The main path reaches every step.
        let main = crate::investigation::steps_on(&inv, &report, &at, None);
        assert!(main.iter().all(|s| s.extras.not_reached.is_none()));
        assert_eq!(main[1].symbol.as_deref(), Some("create"));
    }
}
