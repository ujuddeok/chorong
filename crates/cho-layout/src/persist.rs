//! Saving and loading a workspace per repository, as versioned JSON.
//!
//! Loading never fails: a file from an older or newer Chorong, a damaged file or one that names
//! panes this version does not know is read as far as it can be, repaired until every invariant
//! holds, and each problem is reported as a warning. The worst case is the default layout.
//! An open focus layout is not saved (focus layouts are temporary); the arrangement Restore would
//! return to is, with the reading position applied.

use crate::arrangement::{Arrangement, DockId, Region};
use crate::history::{FocusRecord, HISTORY_LIMIT, RecordV1};
use crate::tree::{NodeId, for_each_group_mut, normalize_in_place, renumber, retain_tabs};
use crate::{PaneContent, PaneId, PaneKind, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The format version this build writes.
/// Version 2 records focus history by pages; version 1 files (one page per record) still load.
pub const FORMAT_VERSION: u64 = 2;

#[derive(Serialize, Deserialize)]
struct SavedPane {
    id: PaneId,
    #[serde(flatten)]
    content: PaneContent,
}

#[derive(Serialize)]
struct Saved<'a> {
    version: u64,
    next_id: u64,
    focused: Option<PaneId>,
    panes: Vec<SavedPane>,
    arrangement: Arrangement,
    focus_history: &'a [FocusRecord],
    /// Most recently focused panes first.
    recent: &'a [PaneId],
}

/// A loaded workspace and what had to be repaired to load it.
#[derive(Debug)]
pub struct Loaded {
    pub workspace: Workspace,
    pub warnings: Vec<String>,
}

impl Workspace {
    /// The saved form: panes, the arrangement Restore would return to, focus history (so it
    /// survives restarts) and recency.
    pub fn to_json(&self) -> String {
        let saved = Saved {
            version: FORMAT_VERSION,
            next_id: self.next_id,
            focused: self.focused,
            panes: self
                .panes
                .iter()
                .map(|(id, c)| SavedPane {
                    id: *id,
                    content: c.clone(),
                })
                .collect(),
            arrangement: self.restored_arrangement(),
            focus_history: &self.history,
            recent: &self.recent,
        };
        serde_json::to_string_pretty(&saved).expect("layout serialises")
    }

    /// Reads a saved layout. Never fails; see the module docs.
    pub fn from_json(json: &str) -> Loaded {
        let mut warnings = Vec::new();
        let value: Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(e) => {
                warnings.push(format!("not a layout file ({e}); using the default layout"));
                return defaults(warnings);
            }
        };
        let Some(obj) = value.as_object() else {
            warnings.push("not a layout file; using the default layout".into());
            return defaults(warnings);
        };
        match obj.get("version").and_then(Value::as_u64) {
            None => warnings.push(format!(
                "no format version; reading it as version {FORMAT_VERSION}"
            )),
            Some(v) if v > FORMAT_VERSION => warnings.push(format!(
                "written by a newer Chorong (format {v}); reading what this version knows"
            )),
            Some(_) => {}
        }

        let mut panes = BTreeMap::new();
        for raw in obj
            .get("panes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match serde_json::from_value::<SavedPane>(raw.clone()) {
                Ok(p) if !panes.contains_key(&p.id) => {
                    panes.insert(p.id, p.content);
                }
                Ok(p) => warnings.push(format!("pane {} is listed twice; kept the first", p.id.0)),
                Err(e) => warnings.push(format!("skipped a pane this version cannot read ({e})")),
            }
        }
        let arrangement = match obj.get("arrangement").cloned() {
            Some(a) => serde_json::from_value::<Arrangement>(a).unwrap_or_else(|e| {
                warnings.push(format!(
                    "could not read the arrangement ({e}); panes go to their default places"
                ));
                Arrangement::default()
            }),
            None => Arrangement::default(),
        };
        let mut history = Vec::new();
        for raw in obj
            .get("focus_history")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let parsed = if raw.get("pages").is_some() {
                serde_json::from_value::<FocusRecord>(raw.clone())
            } else {
                serde_json::from_value::<RecordV1>(raw.clone()).map(FocusRecord::from)
            };
            match parsed {
                Ok(r) => history.push(r),
                Err(e) => warnings.push(format!("skipped a focus history entry ({e})")),
            }
        }
        history.truncate(HISTORY_LIMIT);

        let mut ws = Workspace {
            panes,
            arrangement,
            focus: None,
            history,
            focused: obj
                .get("focused")
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
            next_id: obj.get("next_id").and_then(Value::as_u64).unwrap_or(1),
            recent: obj
                .get("recent")
                .and_then(|v| serde_json::from_value::<Vec<PaneId>>(v.clone()).ok())
                .unwrap_or_default(),
            undo: Default::default(),
            undo_depth: 0,
            all_docks: None,
        };
        repair(&mut ws, &mut warnings);
        if let Err(e) = ws.check_invariants() {
            warnings.push(format!(
                "layout could not be repaired ({e}); using the default layout"
            ));
            return defaults(warnings);
        }
        Loaded {
            workspace: ws,
            warnings,
        }
    }
}

fn defaults(warnings: Vec<String>) -> Loaded {
    Loaded {
        workspace: Workspace::with_defaults(),
        warnings,
    }
}

/// Brings a loaded workspace back within the invariants, noting each repair.
fn repair(ws: &mut Workspace, warnings: &mut Vec<String>) {
    // Drop unknown, repeated and misplaced panes from the trees.
    let mut seen = BTreeSet::new();
    let mut misplaced = Vec::new();
    for r in Region::ALL {
        let panes = &ws.panes;
        if let Some(root) = ws.arrangement.root_mut(r) {
            retain_tabs(root, &mut |p| {
                let Some(content) = panes.get(&p) else {
                    warnings.push(format!("dropped unknown pane {} from the layout", p.0));
                    return false;
                };
                if !seen.insert(p) {
                    warnings.push(format!("pane {} was placed twice; kept the first", p.0));
                    return false;
                }
                let editor = content.kind() == PaneKind::Editor;
                if editor && r != Region::Editor {
                    misplaced.push(p);
                    seen.remove(&p);
                    return false;
                }
                true
            });
            // A tab group holds one kind: tabs of the other kind than its first leave it.
            for_each_group_mut(root, &mut |g| {
                let kind_of = |p: &PaneId| panes.get(p).map(|c| c.kind() == PaneKind::Editor);
                if g.tabs.is_empty() {
                    return;
                }
                let first = kind_of(&g.tabs[0]);
                let (keep, out): (Vec<PaneId>, Vec<PaneId>) =
                    g.tabs.iter().partition(|p| kind_of(p) == first);
                if !out.is_empty() {
                    let active = g.active_pane();
                    g.tabs = keep;
                    g.active = active
                        .and_then(|a| g.tabs.iter().position(|p| *p == a))
                        .unwrap_or(0);
                    for p in out {
                        misplaced.push(p);
                        seen.remove(&p);
                    }
                }
            });
            // Pins follow tool tabs.
            for_each_group_mut(root, &mut |g| {
                let active = g.active_pane();
                g.tabs
                    .sort_by_key(|p| panes.get(p).is_some_and(|c| c.kind().is_pin()));
                g.active = active
                    .and_then(|a| g.tabs.iter().position(|p| *p == a))
                    .unwrap_or(0);
            });
        }
        normalize_in_place(ws.arrangement.root_mut(r));
    }
    if !misplaced.is_empty() {
        warnings.push(format!(
            "{} pane(s) were in a region they cannot be in; moved to their default places",
            misplaced.len()
        ));
    }

    for d in DockId::ALL {
        let (lo, hi) = d.size_range();
        let dock = ws.arrangement.dock_mut(d);
        if !dock.size.is_finite() {
            dock.size = d.default_size();
        }
        dock.size = dock.size.clamp(lo, hi);
    }

    // Ids: the counter must pass every id, and group and split ids must be unique.
    let max_pane = ws.panes.keys().map(|p| p.0).max().unwrap_or(0);
    let nodes = ws.arrangement.node_ids();
    let max_node = nodes.iter().map(|n| n.0).max().unwrap_or(0);
    ws.next_id = ws.next_id.max(max_pane + 1).max(max_node + 1);
    let unique: BTreeSet<NodeId> = nodes.iter().copied().collect();
    let clash =
        unique.len() != nodes.len() || ws.panes.keys().any(|p| unique.contains(&NodeId(p.0)));
    if clash {
        warnings.push("group ids clashed; renumbered them".into());
        for r in Region::ALL {
            if let Some(root) = ws.arrangement.root_mut(r) {
                let next_id = &mut ws.next_id;
                renumber(root, &mut || {
                    let id = NodeId(*next_id);
                    *next_id += 1;
                    id
                });
            }
        }
    }

    // Panes that exist but are not placed go to their default places, in id order.
    let unplaced: Vec<PaneId> = ws
        .panes
        .keys()
        .copied()
        .filter(|p| !seen.contains(p))
        .collect();
    let silently: BTreeSet<PaneId> = misplaced.into_iter().collect();
    let noisy = unplaced.iter().filter(|p| !silently.contains(p)).count();
    if noisy > 0 {
        warnings.push(format!(
            "{noisy} pane(s) had no place in the layout; put them in their default places"
        ));
    }
    for p in unplaced {
        ws.place_default(p);
    }
    let had_editor_area = ws.arrangement.editor.is_some();
    ws.settle_editor_area();
    if had_editor_area && ws.arrangement.editor.is_none() {
        warnings.push("the code area had no code pane; its tool panes went to their docks".into());
    }

    let mut seen_recent = BTreeSet::new();
    let panes = &ws.panes;
    ws.recent
        .retain(|p| panes.contains_key(p) && seen_recent.insert(*p));
    if ws.focused.is_some_and(|p| !ws.panes.contains_key(&p)) {
        ws.focused = None;
    }
}

/// The file a repository's layout is saved in, inside the viewer's state directory. Named by a
/// stable hash of the repository root, so it never contains the path itself.
pub fn layout_file_name(repo_root: &str) -> String {
    format!(
        "layout-{:016x}.json",
        crate::pane::fnv1a(repo_root.as_bytes())
    )
}

/// Saves `ws` for `repo_root` under `dir`, writing a temporary file first so a crash never
/// leaves half a layout.
pub fn save(dir: &Path, repo_root: &str, ws: &Workspace) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(layout_file_name(repo_root));
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, ws.to_json())?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Loads the layout saved for `repo_root` under `dir`; the default layout when there is none.
#[allow(clippy::disallowed_methods)] // Chorong's own layout file
pub fn load(dir: &Path, repo_root: &str) -> Loaded {
    match std::fs::read_to_string(dir.join(layout_file_name(repo_root))) {
        Ok(json) => Workspace::from_json(&json),
        Err(_) => defaults(Vec::new()),
    }
}
