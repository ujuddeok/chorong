//! Diff panes on tour pages (investigation format v2): one file between two revisions, shown
//! with the stage 3 comparison view's marks (`compare::Gutter`, `compare::Words` and the
//! change tints of `change_marks`), never a second diff view. The pane is an editor showing
//! the head side (After); a file the head no longer has shows the base side (Before).
//!
//! The two texts come from `cho_investigation::diff::read`, which resolves both revisions
//! with git through `cho_core::git::command` and refuses names that look like options. A
//! revision that does not resolve never reaches here: the page shows a card saying so.
//! Everything is computed once, in the background, when the page is built.

use super::compare::{Diff, Gutter, Side, Words, decls};
use super::compare_git::FileHunk;
use super::*;
use crate::tour_pages::DiffAt;
use cho_git::diff_lines;
use cho_git::linediff::DEFAULT_MAX_COST;
use std::path::Path;

/// What a diff pane shows, built in the background.
struct Loaded {
    doc: Document,
    side: Side,
    diff: Diff,
    /// Where the caret starts: the first change on the shown side.
    first_change: Option<usize>,
}

/// Reads both sides and diffs them. `file` is the absolute path, `display` how it is named.
fn load(
    root: &Path,
    at: &DiffAt,
    file: PathBuf,
    display: String,
    theme: &cho_theme::Theme,
    json_folds: bool,
) -> Result<Loaded, String> {
    let texts = cho_investigation::diff::read(root, &at.path, &at.base, at.head.as_deref());
    if let Some(problem) = texts.problem(&at.path) {
        return Err(problem);
    }
    let base = texts.base_bytes.clone().unwrap_or_default();
    let head = texts.head_bytes.clone().unwrap_or_default();
    let line_diff = diff_lines(&base, &head, DEFAULT_MAX_COST);
    let hunks: Vec<FileHunk> = line_diff
        .hunks
        .iter()
        .map(|h| FileHunk {
            hunk: h.clone(),
            uncommitted: false,
        })
        .collect();
    let words = if line_diff.coarse || texts.base_bytes.is_none() || texts.head_bytes.is_none() {
        Words::default()
    } else {
        Words::compute(&base, &head, &hunks)
    };
    let short = |c: &Option<String>| c.as_deref().map(|c| cho_git::short(c).to_string());
    // The head side unless the file is gone there.
    let (side, bytes, at_rev) = if texts.head.has_file {
        (Side::After, head, short(&texts.head.commit))
    } else {
        (Side::Before, base, short(&texts.base.commit))
    };
    let first_change = hunks.first().map(|h| match side {
        Side::After => h.hunk.head.start,
        Side::Before => h.hunk.base.start,
    });
    let doc = Document::from_bytes(file, display, bytes, at_rev, theme, json_folds)
        .map_err(|e| e.to_string())?;
    Ok(Loaded {
        doc,
        side,
        diff: Diff::new(hunks, line_diff.coarse, words),
        first_change,
    })
}

impl Chorong {
    /// Shows the diff pane `at` in the active editor: the file on its head side with the
    /// change marks against its base. The step's mark is already set.
    pub(super) fn open_diff(&mut self, at: DiffAt, cx: &mut Context<Self>) {
        let Some(root) = self.inv.as_ref().map(|s| s.root.clone()) else {
            return;
        };
        let file = root.join(&at.path);
        let display = self.display_of(&file);
        let theme = self.theme.clone();
        let json = self.features.on(Feature::JsonFolds);
        self.remember_caret();
        let generation = self.ed_mut().generation.next();
        let pane = self.active_editor;
        let rel = at.path.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { load(&root, &at, file, display, &theme, json) })
                .await;
            this.update(cx, |this, cx| {
                if !this.editor_is(pane, generation) {
                    return;
                }
                match loaded {
                    Ok(l) => {
                        if crate::devlog::on() {
                            eprintln!(
                                "INV diff path={rel} side={} hunks={}",
                                match l.side {
                                    Side::After => "after",
                                    Side::Before => "before",
                                },
                                l.diff.hunks.len()
                            );
                        }
                        this.with_pane(pane, |this| {
                            let doc = Arc::new(l.doc);
                            let at_rev = doc.at_rev.clone();
                            let lines = doc.lines.len();
                            let caret = l
                                .first_change
                                .and_then(|line| doc.buffer.line_range(line))
                                .map(|r| r.start);
                            // The page's own load of the file was superseded by this one.
                            this.ed_mut().loading = None;
                            this.install_arc(doc.clone());
                            let ed = this.ed_mut();
                            if let Some(mark) = &mut ed.step_mark {
                                // The tint and note follow the side shown.
                                mark.at_rev = at_rev;
                            }
                            ed.side = l.side;
                            ed.gutter =
                                Some(Arc::new(Gutter::from_hunks(&l.diff.hunks, l.side, lines)));
                            ed.decls = Some(Arc::new(decls(&doc)));
                            // The changed lines read without a click: open the folds that
                            // hide them.
                            if let Some(folds) = &mut ed.folds {
                                for h in &l.diff.hunks {
                                    let lines = match l.side {
                                        Side::After => h.hunk.head.clone(),
                                        Side::Before => h.hunk.base.clone(),
                                    };
                                    for line in lines.start..lines.end.min(lines.start + 2000) {
                                        folds.reveal(line);
                                    }
                                }
                            }
                            ed.diff = Some(Arc::new(l.diff));
                            this.apply_target(
                                &Target {
                                    byte: caret,
                                    ..Target::default()
                                },
                                cx,
                            );
                            this.record_history();
                        });
                        this.dirty_pane = Some(pane);
                        this.sync_editor_pane(pane, cx);
                        this.refresh_context(cx);
                    }
                    Err(e) => {
                        if let Some(ed) = this.editors.get_mut(&pane) {
                            ed.loading = None;
                            ed.error = Some(e.clone());
                        }
                        this.notify_text(e, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
