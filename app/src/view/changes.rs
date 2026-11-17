//! The Changes list: the declarations a comparison with a branch touched, grouped by file. The
//! source is `cho_changes::compare_worktree` (Before is the comparison's merge base, After the
//! working folder), computed on the background executor when the comparison starts, a file
//! opens or the window is activated (never polled) and only while a Changes pane exists. A
//! result for an older request is dropped by generation.
//!
//! The pure part (rows, order, where a row opens) is `build_rows` and `step`, tested without
//! GPUI. Positions are UTF-8 byte offsets; lines here are zero-based.

use super::row_list::{chevron, filter_box, mark_cell, row_shell, row_tint, tag};
use super::shell_host::{OpenAt, Redraw, pane_of};
use super::*;
use cho_changes::{ChangeSet, FileChanges, FileStatus, SymbolChange, SymbolKey, SymbolStatus};
use cho_layout::PaneKind;
use cho_layout_ui::colors::kind_glyph;
use cho_layout_ui::widgets::small_icon;
use cho_syntax::{Language, Symbol};
use std::path::Path;

const ROW_H: f32 = 22.;

/// Where a row opens: a file on one side of the comparison, at a declaration or a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangePlace {
    /// Repository-relative path of the file in the working folder (the head path), or its path
    /// at the merge base when the file was deleted.
    pub path: String,
    /// The file is gone from the working folder: only Before can show it.
    pub deleted: bool,
    pub side: Side,
    /// The declaration to land on, matched against the side's outline (the key carries the
    /// path on that side: the old path of a renamed file on Before).
    pub key: Option<SymbolKey>,
    /// Which of the declarations sharing `key` (an ambiguous change lists every candidate).
    pub nth: usize,
    /// Without a key: the zero-based line on `side` to land on.
    pub line: Option<usize>,
}

/// How a symbol changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Added,
    Changed,
    Removed,
}

impl Mark {
    /// The mark's shape; its colour comes from the theme, but the glyph says it alone.
    pub fn glyph(self) -> &'static str {
        match self {
            Mark::Added => "+",
            Mark::Changed => "~",
            Mark::Removed => "\u{2212}",
        }
    }
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChangeRow {
    File {
        path: String,
        /// The path at the merge base, for a renamed file.
        old_path: Option<String>,
        /// Added, modified, deleted or renamed since the merge base.
        status: FileStatus,
        /// Only uncommitted work changed this file.
        uncommitted: bool,
        /// Symbol changes, else hunks.
        count: usize,
        counts_hunks: bool,
        place: ChangePlace,
    },
    Symbol {
        mark: Mark,
        kind: &'static str,
        name: String,
        uncommitted: bool,
        place: ChangePlace,
    },
    /// An ambiguous change: several declarations share the name and kind. Not selectable; the
    /// candidates below it are.
    Group {
        kind: &'static str,
        name: String,
        uncommitted: bool,
        candidates: usize,
    },
    Candidate {
        /// Which side the declaration is on.
        side: Side,
        /// One-based line of the declaration on that side.
        line: usize,
        /// Whether changed lines fall inside it.
        touched: bool,
        place: ChangePlace,
    },
}

impl ChangeRow {
    pub fn place(&self) -> Option<&ChangePlace> {
        match self {
            ChangeRow::File { place, .. }
            | ChangeRow::Symbol { place, .. }
            | ChangeRow::Candidate { place, .. } => Some(place),
            ChangeRow::Group { .. } => None,
        }
    }
}

/// The rows for `set`: files by path; in a file, the symbols that live on After (added,
/// changed, ambiguous) in their order there, then the removed ones in their order on Before.
pub(crate) fn build_rows(set: &ChangeSet) -> Vec<ChangeRow> {
    let mut files: Vec<&FileChanges> = set.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut rows = Vec::new();
    for f in files {
        rows.push(file_row(f));
        let mut symbols: Vec<&SymbolChange> = f.symbols.iter().collect();
        symbols.sort_by_cached_key(|s| symbol_order(s));
        for s in symbols {
            symbol_rows(f, s, &mut rows);
        }
    }
    rows
}

/// (lives on Before only, first line on its side, name, kind): the order inside a file.
fn symbol_order(s: &SymbolChange) -> (bool, usize, String, &'static str) {
    let removed = s.status == SymbolStatus::Removed;
    let first = if removed {
        s.base.first()
    } else {
        s.head.first().or(s.base.first())
    };
    (
        removed,
        first.map_or(0, |d| d.start_line),
        s.qualified_name.clone(),
        s.kind.as_str(),
    )
}

fn file_row(f: &FileChanges) -> ChangeRow {
    let deleted = f.status == FileStatus::Deleted;
    let first = f.hunks.first();
    let (side, line) = if deleted {
        (
            Side::Before,
            first.map(|h| h.base.start_line.saturating_sub(1)),
        )
    } else {
        (
            Side::After,
            first.map(|h| h.head.start_line.saturating_sub(1)),
        )
    };
    let counts_hunks = f.symbols.is_empty();
    ChangeRow::File {
        path: f.path.clone(),
        old_path: f.old_path.clone(),
        status: f.status,
        uncommitted: f.uncommitted && !f.hunks.is_empty() && f.hunks.iter().all(|h| h.uncommitted),
        count: if counts_hunks {
            f.hunks.len()
        } else {
            f.symbols.len()
        },
        counts_hunks,
        place: ChangePlace {
            path: f.path.clone(),
            deleted,
            side,
            key: None,
            nth: 0,
            line,
        },
    }
}

/// The place of the `nth` declaration of `s` on `side` of `f`.
fn place_of(f: &FileChanges, s: &SymbolChange, side: Side, nth: usize) -> ChangePlace {
    let cs = match side {
        Side::After => cho_changes::Side::Head,
        Side::Before => cho_changes::Side::Base,
    };
    ChangePlace {
        path: f.path.clone(),
        deleted: f.status == FileStatus::Deleted,
        side,
        key: f.symbol_key(s, cs),
        nth,
        line: None,
    }
}

fn symbol_rows(f: &FileChanges, s: &SymbolChange, rows: &mut Vec<ChangeRow>) {
    if s.status == SymbolStatus::Ambiguous {
        rows.push(ChangeRow::Group {
            kind: s.kind.as_str(),
            name: s.qualified_name.clone(),
            uncommitted: s.uncommitted,
            candidates: s.head.len() + s.base.len(),
        });
        for (i, d) in s.head.iter().enumerate() {
            rows.push(ChangeRow::Candidate {
                side: Side::After,
                line: d.start_line,
                touched: d.touched,
                place: place_of(f, s, Side::After, i),
            });
        }
        for (i, d) in s.base.iter().enumerate() {
            rows.push(ChangeRow::Candidate {
                side: Side::Before,
                line: d.start_line,
                touched: d.touched,
                place: place_of(f, s, Side::Before, i),
            });
        }
        return;
    }
    // A removed symbol exists only on Before; the others open on After.
    let (mark, side) = match s.status {
        SymbolStatus::Added => (Mark::Added, Side::After),
        SymbolStatus::Removed => (Mark::Removed, Side::Before),
        SymbolStatus::Changed | SymbolStatus::Ambiguous => (Mark::Changed, Side::After),
    };
    rows.push(ChangeRow::Symbol {
        mark,
        kind: s.kind.as_str(),
        name: s.qualified_name.clone(),
        uncommitted: s.uncommitted,
        place: place_of(f, s, side, 0),
    });
}

/// The next selectable row on screen (`shown`, indices into `rows`) from `current` in the
/// direction of `delta` (the first or last when nothing on screen is selected). Stays put at
/// either end.
pub(crate) fn step(
    rows: &[ChangeRow],
    shown: &[usize],
    current: Option<usize>,
    delta: isize,
) -> Option<usize> {
    let ok = |p: &usize| rows.get(shown[*p]).is_some_and(|r| r.place().is_some());
    let pos = current.and_then(|c| shown.iter().position(|r| *r == c));
    let found = match (pos, delta >= 0) {
        (None, true) => (0..shown.len()).find(ok),
        (None, false) => (0..shown.len()).rev().find(ok),
        (Some(c), true) => (c + 1..shown.len()).find(ok),
        (Some(c), false) => (0..c).rev().find(ok),
    };
    found.or(pos).map(|p| shown[p])
}

/// The status mark of a file row: one letter, which says the status alone (the colour only
/// repeats it).
pub(crate) fn status_letter(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "A",
        FileStatus::Modified => "M",
        FileStatus::Deleted => "D",
        FileStatus::Renamed => "R",
    }
}

/// The index of the file row `ix` belongs to (itself for a file row).
pub(crate) fn file_of(rows: &[ChangeRow], ix: usize) -> Option<usize> {
    (0..=ix.min(rows.len().checked_sub(1)?))
        .rev()
        .find(|i| matches!(rows[*i], ChangeRow::File { .. }))
}

/// The path of the file row at `ix`, if it is one.
pub(crate) fn file_path(rows: &[ChangeRow], ix: usize) -> Option<&str> {
    match rows.get(ix) {
        Some(ChangeRow::File { path, .. }) => Some(path),
        _ => None,
    }
}

/// Which rows the query matches: file rows by path, declaration rows (and an ambiguous group's
/// candidates, with their group) by qualified name. Fuzzy, the way the file picker matches.
fn query_hits(rows: &[ChangeRow], query: &str) -> Vec<bool> {
    use cho_layout_ui::fuzzy::Ranker;
    let mut hit = vec![false; rows.len()];
    let (mut files, mut names) = (Vec::new(), Vec::new());
    for (i, r) in rows.iter().enumerate() {
        match r {
            ChangeRow::File { path, old_path, .. } => {
                let text = match old_path {
                    Some(old) => format!("{old} {path}"),
                    None => path.clone(),
                };
                files.push((i, text));
            }
            ChangeRow::Symbol { name, .. } | ChangeRow::Group { name, .. } => {
                names.push((i, name.clone()));
            }
            ChangeRow::Candidate { .. } => {}
        }
    }
    let mut mark = |list: &[(usize, String)], mut ranker: Ranker| {
        let texts: Vec<&str> = list.iter().map(|(_, t)| t.as_str()).collect();
        for m in ranker.rank(&texts, query, texts.len()) {
            hit[list[m.index].0] = true;
        }
    };
    mark(&files, Ranker::paths());
    mark(&names, Ranker::labels());
    // A group's candidates follow it.
    let mut group = false;
    for (i, r) in rows.iter().enumerate() {
        match r {
            ChangeRow::Group { .. } => group = hit[i],
            ChangeRow::Candidate { .. } => hit[i] = group,
            _ => group = false,
        }
    }
    hit
}

/// The rows on screen, as indices into `rows`: every file row, and the rows under the files
/// in `expanded`. With a query, a file stays when its path or a declaration in it matches; a
/// collapsed file then shows its matching declarations, an expanded one all of them.
pub(crate) fn visible_rows(
    rows: &[ChangeRow],
    expanded: &HashSet<String>,
    query: &str,
) -> Vec<usize> {
    let query = query.trim();
    let hits = (!query.is_empty()).then(|| query_hits(rows, query));
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let ChangeRow::File { path, .. } = &rows[i] else {
            i += 1;
            continue;
        };
        let end = (i + 1..rows.len())
            .find(|j| matches!(rows[*j], ChangeRow::File { .. }))
            .unwrap_or(rows.len());
        let open = expanded.contains(path);
        match &hits {
            None => {
                out.push(i);
                if open {
                    out.extend(i + 1..end);
                }
            }
            Some(hit) => {
                let inner: Vec<usize> = (i + 1..end).filter(|j| hit[*j]).collect();
                if hit[i] || !inner.is_empty() {
                    out.push(i);
                    if open {
                        out.extend(i + 1..end);
                    } else {
                        out.extend(inner);
                    }
                }
            }
        }
        i = end;
    }
    out
}

/// Whether the file row at `ix` shows the rows under it in `shown`.
pub(crate) fn shows_children(rows: &[ChangeRow], shown: &[usize], ix: usize) -> bool {
    shown
        .iter()
        .position(|r| *r == ix)
        .and_then(|p| shown.get(p + 1))
        .is_some_and(|next| !matches!(rows[*next], ChangeRow::File { .. }))
}

/// The outline entries of a file (as `symbols`) that `key` names, in source order.
pub(crate) fn matching_symbols(
    symbols: &[Symbol],
    language: Language,
    key: &SymbolKey,
) -> Vec<usize> {
    symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| SymbolKey::of(&key.path, s, language) == *key)
        .map(|(i, _)| i)
        .collect()
}

/// Escapes the glob metacharacters in a path, so it matches only itself.
fn glob_escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for ch in path.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '{' | '}' | '\\' | '!') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Search globs that include exactly the changed files in `rows` (paths relative to `top`)
/// that are under the searched folder `root`, each anchored at `root`. A deleted file is not
/// in the folder, so it is left out.
pub(crate) fn changed_globs(rows: &[ChangeRow], top: &Path, root: &Path) -> Vec<String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let top = top.canonicalize().unwrap_or_else(|_| top.to_path_buf());
    rows.iter()
        .filter_map(|r| match r {
            ChangeRow::File { path, status, .. } if *status != FileStatus::Deleted => {
                let rel = top.join(path);
                let rel = rel.strip_prefix(&root).ok()?;
                let rel: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                Some(format!("/{}", glob_escape(&rel.join("/"))))
            }
            _ => None,
        })
        .collect()
}

/// What the list says and shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Status {
    /// No comparison.
    #[default]
    NotComparing,
    Computing,
    Failed(String),
    Ready,
}

pub(crate) struct ChangesState {
    pub status: Status,
    pub rows: Arc<Vec<ChangeRow>>,
    /// The repository's top level, which the rows' paths are relative to.
    pub top: Option<PathBuf>,
    /// What the rows were computed against.
    pub against: String,
    pub generation: Generation,
    /// The window was activated while the list did not refresh by itself: the rows may lag the
    /// working folder.
    pub stale: bool,
    /// The selected row, an index into `rows`.
    pub selected: Option<usize>,
    /// The files whose declarations are listed (by path); the rest are collapsed.
    pub expanded: HashSet<String>,
    /// The filter box: a fuzzy query on paths and declaration names.
    pub query: String,
    /// The whole query is selected (Cmd-A).
    pub query_all: bool,
    /// The rows on screen, as indices into `rows` (`visible_rows`).
    pub shown: Arc<Vec<usize>>,
    pub scroll: UniformListScrollHandle,
    pub over: crate::overscroll::Overscroll,
}

impl Default for ChangesState {
    fn default() -> Self {
        ChangesState {
            status: Status::NotComparing,
            rows: Arc::default(),
            top: None,
            against: String::new(),
            generation: Generation::default(),
            stale: false,
            selected: None,
            expanded: HashSet::new(),
            query: String::new(),
            query_all: false,
            shown: Arc::default(),
            scroll: UniformListScrollHandle::new(),
            over: Default::default(),
        }
    }
}

impl ChangesState {
    /// Works the rows on screen out again (after the rows, the expanded files or the query
    /// changed). A selection no longer on screen moves to its file, when that is.
    pub fn reshow(&mut self) {
        self.shown = Arc::new(visible_rows(&self.rows, &self.expanded, &self.query));
        if let Some(sel) = self.selected
            && !self.shown.contains(&sel)
        {
            self.selected = file_of(&self.rows, sel).filter(|f| self.shown.contains(f));
        }
    }

    /// Expands or collapses the file row at `file` (an index into `rows`).
    fn set_open(&mut self, file: usize, open: bool) {
        if let Some(path) = file_path(&self.rows, file).map(str::to_string) {
            if open {
                self.expanded.insert(path);
            } else {
                self.expanded.remove(&path);
            }
        }
    }

    /// Scrolls the selection into view.
    fn reveal_selected(&self, strategy: ScrollStrategy) {
        if let Some(pos) = self
            .selected
            .and_then(|s| self.shown.iter().position(|r| *r == s))
        {
            self.scroll.scroll_to_item(pos, strategy);
        }
    }
}

impl Chorong {
    /// Whether a Changes pane exists: the list is computed only then.
    fn has_changes_pane(&self, cx: &Context<Self>) -> bool {
        pane_of(self.shell.read(cx).workspace(), PaneKind::Changes).is_some()
    }

    /// The search globs for "Changed files only", or why there are none yet.
    pub(super) fn changed_file_globs(&self) -> Result<Vec<String>, String> {
        if self.compare.is_none() {
            return Err("Changed files only: not comparing".into());
        }
        let (Status::Ready, Some(top), Some(root)) =
            (&self.changes.status, &self.changes.top, &self.root)
        else {
            return Err("Changed files only: waiting for the Changes list".into());
        };
        Ok(changed_globs(&self.changes.rows, top, root))
    }

    /// Redraws the Changes panes only.
    pub(super) fn refresh_changes_panes(&mut self, cx: &mut Context<Self>) {
        self.redraw(Some(Redraw::Changes), cx);
    }

    /// The window was activated: refreshes the list when `changes.auto_refresh` is on (it is
    /// off in Lightweight mode), else only notes that the rows may be out of date.
    pub(super) fn changes_activated(&mut self, cx: &mut Context<Self>) {
        if self.compare.is_none() || !self.has_changes_pane(cx) {
            return;
        }
        if self.features.on(Feature::ChangesAutoRefresh) {
            self.refresh_changes(true, cx);
        } else if self.changes.status == Status::Ready && !self.changes.stale {
            self.changes.stale = true;
            self.refresh_changes_panes(cx);
        }
    }

    /// Computes the list again in the background (when comparing and a Changes pane exists),
    /// and forgets it when not comparing. `automatic` work (window activation) is counted as
    /// the `changes.auto_refresh` feature's cost; one you asked for is yours.
    pub(super) fn refresh_changes(&mut self, automatic: bool, cx: &mut Context<Self>) {
        let generation = self.changes.generation.next();
        self.changes.stale = false;
        let cost = automatic.then(|| self.features.probe(Feature::ChangesAutoRefresh));
        let (Some(cmp), Some(root)) = (self.compare.clone(), self.root.clone()) else {
            let ch = &mut self.changes;
            ch.status = Status::NotComparing;
            ch.rows = Arc::default();
            ch.selected = None;
            ch.expanded.clear();
            ch.reshow();
            self.refresh_changes_panes(cx);
            return;
        };
        if !self.has_changes_pane(cx) {
            return;
        }
        self.changes.status = Status::Computing;
        self.refresh_changes_panes(cx);
        cx.spawn(async move |this, cx| {
            let against = cmp.against.clone();
            let done = cx
                .background_spawn(async move {
                    let _t = cost.as_ref().map(|c| c.background());
                    let started = Instant::now();
                    let top = super::compare_git::repo_top(&root)?;
                    let set = cho_changes::compare_worktree(&top, &cmp.base)?;
                    Ok::<_, String>((top, build_rows(&set), started.elapsed()))
                })
                .await;
            this.update(cx, |this, cx| {
                if !this.changes.generation.is(generation) || this.compare.is_none() {
                    return;
                }
                let ch = &mut this.changes;
                match done {
                    Ok((top, rows, took)) => {
                        if crate::devlog::on() {
                            eprintln!("CHANGES compute_us={}", took.as_micros());
                        }
                        ch.top = Some(top);
                        ch.against = against;
                        ch.selected = ch.selected.filter(|i| *i < rows.len());
                        ch.rows = Arc::new(rows);
                        ch.status = Status::Ready;
                        ch.reshow();
                        if this.search.as_ref().is_some_and(|s| s.changed_only) {
                            this.schedule_search(cx);
                        }
                    }
                    Err(e) => {
                        ch.rows = Arc::default();
                        ch.selected = None;
                        ch.status = Status::Failed(e);
                        ch.reshow();
                    }
                }
                this.refresh_changes_panes(cx);
            })
            .ok();
        })
        .detach();
    }

    /// "Show Changes": reveals the pane (making it, in its kind's default dock, when absent),
    /// puts the keyboard in it and computes the list.
    pub(super) fn show_changes(&mut self, cx: &mut Context<Self>) {
        self.reveal_changes_pane(false, cx);
        self.focus_to(KeyFocus::Changes);
        self.refresh_changes(false, cx);
        let ch = &mut self.changes;
        if ch.selected.is_none() {
            ch.selected = step(&ch.rows, &ch.shown, None, 1);
        }
        self.refresh_changes_panes(cx);
    }

    /// Makes the Changes pane (in its kind's default dock) when absent and brings it to the
    /// front of its group. With `keep_focus` (a comparison starting) the focused pane stays
    /// focused.
    pub(super) fn reveal_changes_pane(&mut self, keep_focus: bool, cx: &mut Context<Self>) {
        let existing = pane_of(self.shell.read(cx).workspace(), PaneKind::Changes);
        let made = existing.is_none();
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| {
                let pane = match existing {
                    Some(p) => p,
                    None => ws.open(cho_layout::PaneContent::Changes, None)?,
                };
                if !ws.arrangement().dock(cho_layout::DockId::Left).visible && made {
                    let _ = ws.toggle_dock(cho_layout::DockId::Left);
                }
                let was = ws.focused();
                ws.activate(pane)?;
                match was.filter(|p| keep_focus && *p != pane) {
                    Some(p) => ws.activate(p),
                    None => Ok(()),
                }
            });
        });
    }

    /// Up / Down in the list.
    pub(super) fn changes_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ch = &mut self.changes;
        ch.selected = step(&ch.rows, &ch.shown, ch.selected, delta);
        ch.reveal_selected(step_strategy(delta));
        self.refresh_changes_panes(cx);
        // Only the list changed: the action's repaint need not redraw every pane.
        self.skip_refresh = true;
    }

    /// Right (`open`) or Left. Right on a collapsed file expands it, on an expanded one moves to
    /// its first declaration. Left on a declaration moves to its file, on an expanded file
    /// collapses it.
    pub(super) fn changes_expand(&mut self, open: bool, cx: &mut Context<Self>) {
        let ch = &mut self.changes;
        let Some(sel) = ch.selected else { return };
        let is_file = file_path(&ch.rows, sel).is_some();
        let showing = is_file && shows_children(&ch.rows, &ch.shown, sel);
        match (open, is_file) {
            (true, true) if !showing => ch.set_open(sel, true),
            (true, true) => ch.selected = step(&ch.rows, &ch.shown, Some(sel), 1),
            (true, false) => {}
            (false, true) => ch.set_open(sel, false),
            (false, false) => ch.selected = file_of(&ch.rows, sel),
        }
        ch.reshow();
        ch.reveal_selected(ScrollStrategy::Top);
        self.refresh_changes_panes(cx);
        self.skip_refresh = true;
    }

    /// The chevron of the file row at `ix` was clicked: expands or collapses it.
    fn changes_toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ch = &mut self.changes;
        let showing = shows_children(&ch.rows, &ch.shown, ix);
        ch.set_open(ix, !showing);
        ch.reshow();
        self.refresh_changes_panes(cx);
    }

    /// "Expand All Changes" (`open`) and "Collapse All Changes".
    pub(super) fn changes_expand_all(&mut self, open: bool, cx: &mut Context<Self>) {
        let ch = &mut self.changes;
        ch.expanded = if open {
            (0..ch.rows.len())
                .filter_map(|i| file_path(&ch.rows, i).map(str::to_string))
                .collect()
        } else {
            HashSet::new()
        };
        ch.reshow();
        self.refresh_changes_panes(cx);
    }

    /// A key while the list has the keyboard: typing edits the filter box. Returns what the
    /// key did to it.
    pub(super) fn changes_filter_key(
        &mut self,
        k: &gpui::Keystroke,
        paste: Option<&str>,
    ) -> FieldEdit {
        let ch = &mut self.changes;
        let edit = edit_field(k, &mut ch.query, &mut ch.query_all, paste);
        if edit == FieldEdit::Text {
            ch.reshow();
            // The first match is selected, so Enter opens it.
            if ch.selected.is_none() || !ch.query.trim().is_empty() {
                ch.selected = step(&ch.rows, &ch.shown, None, 1);
            }
            ch.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        edit
    }

    /// Esc: clears the filter box, else the keyboard goes back to the code.
    pub(super) fn changes_leave(&mut self, cx: &mut Context<Self>) {
        let ch = &mut self.changes;
        if !ch.query.is_empty() {
            ch.query.clear();
            ch.query_all = false;
            ch.reshow();
            ch.reveal_selected(ScrollStrategy::Center);
            self.refresh_changes_panes(cx);
            self.skip_refresh = true;
            return;
        }
        self.key_focus.leave(KeyFocus::Changes);
        self.refresh_changes_panes(cx);
        self.skip_refresh = true;
    }

    /// The selected row at `place` (Enter is [`OpenAt::Here`]).
    fn changes_open_at(&mut self, place: OpenAt, cx: &mut Context<Self>) {
        if let Some(ix) = self.changes.selected {
            self.prepare_place(place, cx);
            self.changes_open(ix, cx);
        }
    }

    /// Enter or a click: opens the row in the active editor pane. The list keeps the keyboard.
    pub(super) fn changes_open(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(place) = self
            .changes
            .rows
            .get(ix)
            .and_then(ChangeRow::place)
            .cloned()
        else {
            return;
        };
        self.changes.selected = Some(ix);
        if file_path(&self.changes.rows, ix).is_some() {
            // A file opened from its row shows its declarations too.
            self.changes.set_open(ix, true);
            self.changes.reshow();
        }
        self.focus_to(KeyFocus::Changes);
        match (&self.compare, self.changes.top.clone()) {
            (Some(_), Some(top)) => self.open_place(top.join(&place.path), place, cx),
            _ => self.notify_text("Not comparing: use Compare Against\u{2026} first", cx),
        }
        self.refresh_changes_panes(cx);
    }

    /// The Refresh button in the pane's header.
    pub(super) fn changes_refresh_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        div()
            .id("changes-refresh")
            .px(px(6.))
            .h(px(20.))
            .flex()
            .items_center()
            .gap_1()
            .rounded(px(4.))
            .cursor_pointer()
            .text_size(px(11.5))
            .text_color(c.text_muted)
            .hover(|s| s.bg(c.hover).text_color(c.text))
            .active(|s| s.bg(c.ghost_active))
            .tooltip(tip(
                "Compute the list again against the working folder".to_string()
            ))
            .child(small_icon("rotate-ccw", c.icon))
            .child("Refresh")
            .on_click(cx.listener(|this, _, _, cx| {
                this.refresh_changes(false, cx);
            }))
            .into_any_element()
    }

    pub(super) fn render_changes(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let ch = &self.changes;
        let empty = ch.rows.is_empty();
        let line = match &ch.status {
            Status::NotComparing => Some(
                "Not comparing. Use Compare Against\u{2026} to list the changes against a branch."
                    .to_string(),
            ),
            Status::Failed(e) => Some(e.clone()),
            Status::Computing => Some("Computing changes\u{2026}".to_string()),
            Status::Ready if empty => Some(format!("No changes against {}", ch.against)),
            Status::Ready if ch.stale => {
                Some("May be out of date: the window was activated. Press Refresh.".to_string())
            }
            Status::Ready => None,
        };
        let muted = c.text_muted;
        let (added, changed, removed) = (
            c.success.unwrap_or(c.text_accent),
            c.warning.unwrap_or(c.text_accent),
            c.error.unwrap_or(c.text),
        );
        let focused = self.key_focus == KeyFocus::Changes;
        let filtered_out = !empty && ch.shown.is_empty();
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .pt_1()
            .text_size(px(12.5))
            .when_some(line, |d, text| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(11.5))
                        .text_color(muted)
                        .child(text),
                )
            })
            .when(!empty, |d| d.child(self.changes_filter_box(focused, cx)))
            .when(filtered_out, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(11.5))
                        .text_color(muted)
                        .child(format!(
                            "Nothing matches \u{201c}{}\u{201d}",
                            ch.query.trim()
                        )),
                )
            })
            .when(!empty, |d| {
                d.child(
                    crate::scroll::uniform_list(
                        "changes",
                        ch.shown.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _w, cx| {
                            let ch = &this.changes;
                            range
                                .filter_map(|pos| {
                                    let ix = *ch.shown.get(pos)?;
                                    let row = ch.rows.get(ix)?;
                                    let selected = ch.selected == Some(ix);
                                    let base = row_shell(
                                        ("change", ix),
                                        ROW_H,
                                        row_tint(selected, false),
                                        None,
                                        row.place().is_some(),
                                        &c,
                                    );
                                    let tag = |text: &'static str| tag(text, &c);
                                    let el = match row {
                                        ChangeRow::File {
                                            path,
                                            old_path,
                                            status,
                                            uncommitted,
                                            count,
                                            counts_hunks,
                                            ..
                                        } => {
                                            let name = match old_path {
                                                Some(old) => {
                                                    format!("{old} \u{2192} {path}")
                                                }
                                                None => path.clone(),
                                            };
                                            let pill = if *counts_hunks {
                                                format!(
                                                    "{count} hunk{}",
                                                    if *count == 1 { "" } else { "s" }
                                                )
                                            } else {
                                                count.to_string()
                                            };
                                            let open = shows_children(&ch.rows, &ch.shown, ix);
                                            let tone = match status {
                                                FileStatus::Added => added,
                                                FileStatus::Modified => changed,
                                                FileStatus::Deleted => removed,
                                                FileStatus::Renamed => c.text_accent,
                                            };
                                            let chevron =
                                                chevron(("change-chevron", ix), open, ROW_H, &c)
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.changes_toggle(ix, cx);
                                                        },
                                                    ));
                                            base.pl_1()
                                                .font_weight(FontWeight::MEDIUM)
                                                .cursor_pointer()
                                                .child(chevron)
                                                .child(
                                                    mark_cell(status_letter(*status), tone)
                                                        .font_family(c.mono_font.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .child(name),
                                                )
                                                .when(*uncommitted, |d| d.child(tag("uncommitted")))
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .ml_2()
                                                        .px_1p5()
                                                        .rounded_full()
                                                        .bg(c.hover)
                                                        .text_size(px(10.5))
                                                        .font_weight(FontWeight::NORMAL)
                                                        .text_color(muted)
                                                        .child(pill),
                                                )
                                        }
                                        ChangeRow::Symbol {
                                            mark,
                                            kind,
                                            name,
                                            uncommitted,
                                            ..
                                        } => {
                                            let colour = match mark {
                                                Mark::Added => added,
                                                Mark::Changed => changed,
                                                Mark::Removed => removed,
                                            };
                                            base.pl_6()
                                                .cursor_pointer()
                                                .child(mark_cell(mark.glyph(), colour))
                                                .child(div().mr(px(6.)).flex().child(small_icon(
                                                    kind_glyph(kind),
                                                    c.kind(kind),
                                                )))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .child(name.clone()),
                                                )
                                                .when(*uncommitted, |d| d.child(tag("uncommitted")))
                                        }
                                        ChangeRow::Group {
                                            kind,
                                            name,
                                            uncommitted,
                                            ..
                                        } => {
                                            base.pl_6()
                                                .child(mark_cell(Mark::Changed.glyph(), changed))
                                                .child(div().mr(px(6.)).flex().child(small_icon(
                                                    kind_glyph(kind),
                                                    c.kind(kind),
                                                )))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .child(name.clone()),
                                                )
                                                .child(tag("ambiguous"))
                                                .when(*uncommitted, |d| d.child(tag("uncommitted")))
                                        }
                                        ChangeRow::Candidate {
                                            side,
                                            line,
                                            touched,
                                            ..
                                        } => {
                                            let side = match side {
                                                Side::After => "After",
                                                Side::Before => "Before",
                                            };
                                            base.pl(px(64.))
                                                .cursor_pointer()
                                                .text_color(muted)
                                                .child(format!("{side} \u{b7} line {line}"))
                                                .when(*touched, |d| d.child(tag("changed")))
                                        }
                                    };
                                    Some(if row.place().is_some() {
                                        el.on_click(cx.listener(
                                            move |this, e: &gpui::ClickEvent, _, cx| {
                                                this.prepare_place(
                                                    OpenAt::of_click(&e.modifiers()),
                                                    cx,
                                                );
                                                this.changes_open(ix, cx);
                                            },
                                        ))
                                        .into_any_element()
                                    } else {
                                        el.into_any_element()
                                    })
                                })
                                .collect()
                        }),
                    )
                    .wrap(
                        &ch.over,
                        &ch.scroll,
                        self.features.probe_if_on(Feature::Overscroll),
                    )
                    .w_full()
                    .flex_1(),
                )
            })
            .into_any_element()
    }
}

impl Chorong {
    /// The filter box at the top of the Changes list: a fuzzy match on file paths and
    /// declaration names, typed while the list has the keyboard.
    fn changes_filter_box(&self, focused: bool, cx: &mut Context<Self>) -> gpui::Div {
        let c = crate::pane_style::colors(&self.theme, cx);
        let ch = &self.changes;
        div().flex_none().mx_2().mb_1().child(
            filter_box(
                "changes-filter",
                self.query_text(&ch.query, ch.query_all),
                ch.query.is_empty(),
                "Filter files and symbols",
                focused,
                &c,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.focus_to(KeyFocus::Changes);
                this.refresh_changes_panes(cx);
            })),
        )
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        ChangesClose,
        // Left / Right: collapse or expand a file (or move to its file or first declaration).
        ChangesCollapse,
        ChangesExpand,
        ChangesNext,
        ChangesOpen,
        // Cmd-Shift-Enter, Cmd-Enter, Alt-Enter: the row in a new code pane below, to the
        // right, or as a new tab.
        ChangesOpenBelow,
        ChangesOpenRight,
        ChangesOpenTab,
        ChangesPrev,
        CollapseAllChanges,
        ExpandAllChanges,
        ShowChanges
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c("chorong::ShowChanges", "Show Changes", Layer::Compare, 171),
    c(
        "chorong::ExpandAllChanges",
        "Expand All Changes",
        Layer::Compare,
        307,
    ),
    c(
        "chorong::CollapseAllChanges",
        "Collapse All Changes",
        Layer::Compare,
        308,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, ShowChanges, |this, _w, cx| this.show_changes(cx));
    let d = on!(d, cx, ChangesPrev, |this, _w, cx| this.changes_step(-1, cx));
    let d = on!(d, cx, ChangesNext, |this, _w, cx| this.changes_step(1, cx));
    let d = on!(d, cx, ChangesClose, |this, _w, cx| this.changes_leave(cx));
    let d = on!(d, cx, ChangesExpand, |this, _w, cx| this
        .changes_expand(true, cx));
    let d = on!(d, cx, ChangesCollapse, |this, _w, cx| this
        .changes_expand(false, cx));
    let d = on!(d, cx, ExpandAllChanges, |this, _w, cx| this
        .changes_expand_all(true, cx));
    let d = on!(d, cx, CollapseAllChanges, |this, _w, cx| this
        .changes_expand_all(false, cx));
    let d = on!(d, cx, ChangesOpenBelow, |this, _w, cx| this
        .changes_open_at(OpenAt::Below, cx));
    let d = on!(d, cx, ChangesOpenRight, |this, _w, cx| this
        .changes_open_at(OpenAt::Right, cx));
    let d = on!(d, cx, ChangesOpenTab, |this, _w, cx| this
        .changes_open_at(OpenAt::Tab, cx));
    on!(d, cx, ChangesOpen, |this, _w, cx| this
        .changes_open_at(OpenAt::Here, cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_changes::{Decl, Hunk, Span};
    use cho_syntax::SymbolKind;

    fn decl(line: usize, touched: bool) -> Decl {
        Decl {
            range: [0, 1],
            name_range: [0, 1],
            start_line: line,
            end_line: line + 1,
            touched,
        }
    }

    fn sym(status: SymbolStatus, name: &str, base: Vec<Decl>, head: Vec<Decl>) -> SymbolChange {
        SymbolChange {
            qualified_name: name.into(),
            kind: SymbolKind::Function,
            status,
            base,
            head,
            uncommitted: false,
        }
    }

    fn hunk(base_start: usize, head_start: usize, uncommitted: bool) -> Hunk {
        let span = |s| Span {
            start_line: s,
            line_count: 1,
            bytes: [0, 1],
        };
        Hunk {
            base: span(base_start),
            head: span(head_start),
            uncommitted,
        }
    }

    fn file(path: &str, status: FileStatus, symbols: Vec<SymbolChange>) -> FileChanges {
        FileChanges {
            status,
            path: path.into(),
            old_path: None,
            similarity: None,
            kind: cho_changes::EntryKind::File,
            language: Some(Language::Rust),
            level: cho_changes::Level::Symbol,
            outside_declarations: false,
            coarse: false,
            uncommitted: false,
            hunks: vec![hunk(3, 5, false)],
            symbols,
        }
    }

    fn set(files: Vec<FileChanges>) -> ChangeSet {
        ChangeSet {
            version: 1,
            base: "b".into(),
            head: "h".into(),
            merge_base: None,
            worktree: true,
            files,
            notes: Vec::new(),
        }
    }

    #[test]
    fn files_sort_by_path_and_symbols_by_their_side_then_position() {
        let f = file(
            "src/b.rs",
            FileStatus::Modified,
            vec![
                sym(SymbolStatus::Removed, "gone", vec![decl(2, true)], vec![]),
                sym(
                    SymbolStatus::Changed,
                    "late",
                    vec![decl(9, true)],
                    vec![decl(30, true)],
                ),
                sym(SymbolStatus::Added, "early", vec![], vec![decl(10, true)]),
            ],
        );
        let rows = build_rows(&set(vec![
            file("src/a.rs", FileStatus::Modified, vec![]),
            f,
        ]));
        let names: Vec<String> = rows
            .iter()
            .map(|r| match r {
                ChangeRow::File { path, .. } => path.clone(),
                ChangeRow::Symbol { name, .. } => name.clone(),
                _ => "?".into(),
            })
            .collect();
        assert_eq!(names, ["src/a.rs", "src/b.rs", "early", "late", "gone"]);
    }

    #[test]
    fn a_file_without_symbols_counts_hunks_and_opens_at_its_first_hunk() {
        let rows = build_rows(&set(vec![file("README.md", FileStatus::Modified, vec![])]));
        let ChangeRow::File {
            count,
            counts_hunks,
            place,
            ..
        } = &rows[0]
        else {
            panic!()
        };
        assert_eq!((*count, *counts_hunks), (1, true));
        assert_eq!((place.side, place.line), (Side::After, Some(4)));
    }

    #[test]
    fn removed_opens_on_before_with_the_base_path_of_a_rename() {
        let mut f = file(
            "src/new.rs",
            FileStatus::Renamed,
            vec![
                sym(SymbolStatus::Removed, "old_fn", vec![decl(4, true)], vec![]),
                sym(
                    SymbolStatus::Changed,
                    "kept",
                    vec![decl(8, true)],
                    vec![decl(8, true)],
                ),
            ],
        );
        f.old_path = Some("src/old.rs".into());
        let rows = build_rows(&set(vec![f]));
        let ChangeRow::Symbol { place: kept, .. } = &rows[1] else {
            panic!()
        };
        assert_eq!(kept.side, Side::After);
        assert_eq!(kept.key.as_ref().unwrap().path, "src/new.rs");
        let ChangeRow::Symbol {
            place: gone, mark, ..
        } = &rows[2]
        else {
            panic!()
        };
        assert_eq!(*mark, Mark::Removed);
        assert_eq!(gone.side, Side::Before);
        // The working file loads from the new path; the key names the declaration at the old one.
        assert_eq!(gone.path, "src/new.rs");
        assert_eq!(gone.key.as_ref().unwrap().path, "src/old.rs");
    }

    #[test]
    fn a_deleted_file_opens_on_before_at_its_base_path() {
        let rows = build_rows(&set(vec![file("src/x.rs", FileStatus::Deleted, vec![])]));
        let ChangeRow::File { place, .. } = &rows[0] else {
            panic!()
        };
        assert!(place.deleted);
        assert_eq!((place.side, place.line), (Side::Before, Some(2)));
    }

    #[test]
    fn an_ambiguous_change_lists_every_candidate() {
        let f = file(
            "src/a.rs",
            FileStatus::Modified,
            vec![sym(
                SymbolStatus::Ambiguous,
                "dup",
                vec![decl(3, false)],
                vec![decl(5, true), decl(20, false)],
            )],
        );
        let rows = build_rows(&set(vec![f]));
        assert!(matches!(rows[1], ChangeRow::Group { candidates: 3, .. }));
        let cands: Vec<(Side, usize, usize)> = rows[2..]
            .iter()
            .map(|r| match r {
                ChangeRow::Candidate {
                    side, line, place, ..
                } => (*side, *line, place.nth),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            cands,
            [
                (Side::After, 5, 0),
                (Side::After, 20, 1),
                (Side::Before, 3, 0)
            ]
        );
        // The group is not selectable; stepping skips it.
        let all: Vec<usize> = (0..rows.len()).collect();
        assert_eq!(step(&rows, &all, None, 1), Some(0));
        assert_eq!(step(&rows, &all, Some(0), 1), Some(2));
    }

    #[test]
    fn only_uncommitted_files_are_labelled() {
        let mut f = file("a.rs", FileStatus::Modified, vec![]);
        f.uncommitted = true;
        f.hunks = vec![hunk(1, 1, true)];
        let mut g = file("b.rs", FileStatus::Modified, vec![]);
        g.uncommitted = true;
        g.hunks = vec![hunk(1, 1, true), hunk(9, 9, false)];
        let rows = build_rows(&set(vec![f, g]));
        let flags: Vec<bool> = rows
            .iter()
            .filter_map(|r| match r {
                ChangeRow::File { uncommitted, .. } => Some(*uncommitted),
                _ => None,
            })
            .collect();
        assert_eq!(flags, [true, false]);
    }

    #[test]
    fn stepping_stays_at_the_ends() {
        let rows = build_rows(&set(vec![file(
            "a.rs",
            FileStatus::Modified,
            vec![sym(SymbolStatus::Added, "x", vec![], vec![decl(1, true)])],
        )]));
        let all: Vec<usize> = (0..rows.len()).collect();
        assert_eq!(step(&rows, &all, None, -1), Some(1));
        assert_eq!(step(&rows, &all, Some(1), 1), Some(1));
        assert_eq!(step(&rows, &all, Some(0), -1), Some(0));
        assert_eq!(step(&[], &[], None, 1), None);
        // Only rows on screen are stepped to.
        assert_eq!(step(&rows, &[0], Some(0), 1), Some(0));
    }

    /// Two files: `src/net/client.rs` with `connect` and `retry_send`, and `README.md`.
    fn two_files() -> Vec<ChangeRow> {
        build_rows(&set(vec![
            file(
                "src/net/client.rs",
                FileStatus::Modified,
                vec![
                    sym(SymbolStatus::Added, "connect", vec![], vec![decl(1, true)]),
                    sym(
                        SymbolStatus::Changed,
                        "retry_send",
                        vec![decl(9, true)],
                        vec![decl(9, true)],
                    ),
                ],
            ),
            file("README.md", FileStatus::Added, vec![]),
        ]))
    }

    fn names(rows: &[ChangeRow], shown: &[usize]) -> Vec<String> {
        shown
            .iter()
            .map(|i| match &rows[*i] {
                ChangeRow::File { path, .. } => path.clone(),
                ChangeRow::Symbol { name, .. } => format!("  {name}"),
                _ => "?".into(),
            })
            .collect()
    }

    #[test]
    fn files_start_collapsed_and_expand_one_at_a_time() {
        let rows = two_files();
        let none = HashSet::new();
        assert_eq!(
            names(&rows, &visible_rows(&rows, &none, "")),
            ["README.md", "src/net/client.rs"]
        );
        let open: HashSet<String> = ["src/net/client.rs".to_string()].into();
        let shown = visible_rows(&rows, &open, "");
        assert_eq!(
            names(&rows, &shown),
            [
                "README.md",
                "src/net/client.rs",
                "  connect",
                "  retry_send"
            ]
        );
        assert!(shows_children(&rows, &shown, 1));
        assert!(!shows_children(&rows, &shown, 0));
        assert_eq!(file_of(&rows, 3), Some(1));
        assert_eq!(file_of(&rows, 0), Some(0));
    }

    #[test]
    fn the_filter_keeps_files_whose_path_or_declarations_match() {
        let rows = two_files();
        let none = HashSet::new();
        // A declaration matches: its file stays and shows only the matching declarations.
        let shown = visible_rows(&rows, &none, "retry");
        assert_eq!(names(&rows, &shown), ["src/net/client.rs", "  retry_send"]);
        // Fuzzy, like the file picker: a subsequence matches.
        assert_eq!(
            names(&rows, &visible_rows(&rows, &none, "rtsnd")),
            ["src/net/client.rs", "  retry_send"]
        );
        // A path matches: the file stays, collapsed.
        assert_eq!(
            names(&rows, &visible_rows(&rows, &none, "readme")),
            ["README.md"]
        );
        // An expanded file shows all its declarations while it matches.
        let open: HashSet<String> = ["src/net/client.rs".to_string()].into();
        assert_eq!(
            names(&rows, &visible_rows(&rows, &open, "retry")),
            ["src/net/client.rs", "  connect", "  retry_send"]
        );
        assert!(visible_rows(&rows, &none, "zzzz").is_empty());
    }

    #[test]
    fn a_matching_group_brings_its_candidates() {
        let rows = build_rows(&set(vec![file(
            "a.rs",
            FileStatus::Modified,
            vec![
                sym(
                    SymbolStatus::Ambiguous,
                    "dup",
                    vec![decl(3, false)],
                    vec![decl(5, true)],
                ),
                sym(SymbolStatus::Added, "other", vec![], vec![decl(9, true)]),
            ],
        )]));
        let shown = visible_rows(&rows, &HashSet::new(), "dup");
        assert_eq!(shown, [0, 1, 2, 3]);
    }

    #[test]
    fn file_rows_carry_a_status_letter() {
        let rows = two_files();
        let letters: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                ChangeRow::File { status, .. } => Some(status_letter(*status)),
                _ => None,
            })
            .collect();
        assert_eq!(letters, ["A", "M"]);
    }

    #[test]
    fn changed_globs_are_anchored_escaped_and_skip_deleted_files() {
        let rows = build_rows(&set(vec![
            file("app/src/[id].rs", FileStatus::Modified, vec![]),
            file("app/gone.rs", FileStatus::Deleted, vec![]),
            file("docs/a.md", FileStatus::Added, vec![]),
        ]));
        let top = Path::new("/nonexistent-repo");
        assert_eq!(
            changed_globs(&rows, top, top),
            ["/app/src/\\[id\\].rs", "/docs/a.md"]
        );
        // Searching a subfolder: paths are relative to it, and files outside are left out.
        assert_eq!(
            changed_globs(&rows, top, &top.join("app")),
            ["/src/\\[id\\].rs"]
        );
    }

    #[test]
    fn symbol_keys_match_by_name_and_kind() {
        let s = |name: &str, scope: &[&str], kind, at: usize| Symbol {
            name: name.into(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            kind,
            name_range: at..at + 1,
            range: at..at + 10,
        };
        let symbols = [
            s("Queue", &[], SymbolKind::Struct, 0),
            s("push", &["Queue"], SymbolKind::Method, 20),
            s("push", &["Queue"], SymbolKind::Method, 60),
            s("push", &["Queue"], SymbolKind::Function, 90),
        ];
        let lang = Language::Rust;
        let key = |path: &str, name: &str, kind: SymbolKind| SymbolKey {
            path: path.into(),
            qualified_name: name.into(),
            kind,
        };
        let one = matching_symbols(&symbols, lang, &key("a.rs", "Queue", SymbolKind::Struct));
        assert_eq!(one, [0]);
        // Ambiguous: both declarations, in source order, told apart by `nth`.
        let two = matching_symbols(
            &symbols,
            lang,
            &key("a.rs", "Queue::push", SymbolKind::Method),
        );
        assert_eq!(two, [1, 2]);
        // Same name, other kind: not matched.
        let other = matching_symbols(
            &symbols,
            lang,
            &key("a.rs", "Queue::push", SymbolKind::Function),
        );
        assert_eq!(other, [3]);
        assert!(
            matching_symbols(
                &symbols,
                lang,
                &key("a.rs", "Queue::pop", SymbolKind::Method)
            )
            .is_empty()
        );
    }
}
