//! Toggle Blame: a column beside the gutter saying who last changed each line, and when.
//!
//! The answer for a pane is computed in the background by `crate::blame::compute` (git
//! blame on the exact text the pane shows, from `HEAD` or the revision the pane shows), and
//! kept with the pane's `epoch`, so a different document never shows another's blame. The
//! label, shading and hover text are in `crate::blame`.

use super::*;
use crate::blame::{BlameView, COLUMN_CELLS};

/// The blame of one pane's document.
#[derive(Clone)]
pub(crate) struct PaneBlame {
    /// The pane's `epoch` this answer is for.
    pub epoch: u64,
    pub state: BlameState,
    /// Asked again (the window came back to the front: `HEAD` may have moved); the current
    /// state stays on screen until the new answer arrives.
    pub recheck: bool,
}

#[derive(Clone)]
pub(crate) enum BlameState {
    Loading,
    Ready(Arc<BlameView>),
    /// git could not blame this text (no repository, a path that is not at the revision).
    Unavailable(String),
}

/// Space around the label, inside the column.
const PAD: f32 = 6.;

impl Chorong {
    /// Whether `ed` shows the blame column: Toggle Blame is on in that pane and the setting
    /// allows it.
    pub(super) fn blame_shown(&self, ed: &EditorState) -> bool {
        ed.blame_on && self.features.on(Feature::Blame)
    }

    pub(super) fn toggle_blame(&mut self, cx: &mut Context<Self>) {
        if !self.features.on(Feature::Blame) {
            let why = if self.features.store().effective().lightweight() {
                "Blame is off in Lightweight mode"
            } else {
                "Blame is off (the blame.enabled setting)"
            };
            self.notify_text(why, cx);
            return;
        }
        // Only the code pane in use: each pane keeps its own switch.
        let ed = self.ed_mut();
        ed.blame_on = !ed.blame_on;
        self.touch_pane();
        self.ensure_blame(cx);
    }

    /// The window is back in front: a commit made meanwhile changes what is uncommitted, so
    /// panes on the checkout ask again (answered from the cache when `HEAD` has not moved).
    pub(super) fn blame_activated(&mut self, cx: &mut Context<Self>) {
        if !self.features.on(Feature::Blame) {
            return;
        }
        for ed in self.editors.values_mut() {
            if ed.blame_on
                && ed.doc.as_ref().is_some_and(|d| d.at_rev.is_none())
                && let Some(b) = ed.blame.as_mut()
            {
                b.recheck = true;
            }
        }
        self.ensure_blame(cx);
    }

    /// Starts the blame of every editor pane with blame on whose document has none yet (or is
    /// asked again). Cheap when there is nothing to do: called after every change.
    pub(super) fn ensure_blame(&mut self, cx: &mut Context<Self>) {
        if !self.features.on(Feature::Blame) {
            return;
        }
        let wanted: Vec<(PaneId, u64, Arc<Document>)> = self
            .editors
            .iter()
            .filter(|(_, ed)| ed.blame_on && ed.loading.is_none())
            .filter(|(_, ed)| {
                ed.blame
                    .as_ref()
                    .is_none_or(|b| b.epoch != ed.epoch || b.recheck)
            })
            .filter_map(|(pane, ed)| Some((*pane, ed.epoch, ed.doc.clone()?)))
            .collect();
        for (pane, epoch, doc) in wanted {
            if let Some(ed) = self.editors.get_mut(&pane) {
                match ed.blame.as_mut().filter(|b| b.epoch == epoch) {
                    Some(b) => b.recheck = false,
                    None => {
                        ed.blame = Some(PaneBlame {
                            epoch,
                            state: BlameState::Loading,
                            recheck: false,
                        })
                    }
                }
            }
            let cache = self.blame_cache.clone();
            let probe = self.features.probe_if_on(Feature::Blame);
            cx.spawn(async move |this, cx| {
                let done = cx
                    .background_spawn(async move {
                        crate::blame::compute(
                            &cache,
                            &doc.path,
                            doc.at_rev.as_deref(),
                            doc.buffer.text().as_bytes(),
                            probe.as_ref(),
                        )
                    })
                    .await;
                this.update(cx, |this, cx| {
                    let Some(ed) = this.editors.get_mut(&pane) else {
                        return;
                    };
                    if ed.epoch != epoch {
                        // Another document arrived meanwhile: blame that one instead.
                        this.ensure_blame(cx);
                        return;
                    }
                    let recheck = ed.blame.as_ref().is_some_and(|b| b.recheck);
                    ed.blame = Some(PaneBlame {
                        epoch,
                        state: match done {
                            Ok(view) => BlameState::Ready(view),
                            Err(e) => BlameState::Unavailable(e),
                        },
                        recheck,
                    });
                    this.dirty_pane = Some(pane);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The blame state of `ed`'s document while the column is showing.
    pub(super) fn blame_of<'a>(&self, ed: &'a EditorState) -> Option<&'a BlameState> {
        if !self.blame_shown(ed) {
            return None;
        }
        ed.blame
            .as_ref()
            .filter(|b| b.epoch == ed.epoch)
            .map(|b| &b.state)
    }

    /// What the pane header says about blame: nothing once it is drawn.
    pub(super) fn blame_status(&self, ed: &EditorState) -> Option<String> {
        match self.blame_of(ed)? {
            BlameState::Loading => Some("Blame: running git blame…".into()),
            BlameState::Ready(_) => None,
            BlameState::Unavailable(why) => Some(format!("Blame isn't available here: {why}")),
        }
    }

    /// Width of the column for a font cell `advance` wide, while it takes space (an answer
    /// that is coming or has come).
    pub(super) fn blame_width(&self, ed: &EditorState, advance: f32) -> Option<f32> {
        match self.blame_of(ed)? {
            BlameState::Loading | BlameState::Ready(_) => Some(advance * COLUMN_CELLS + 2. * PAD),
            BlameState::Unavailable(_) => None,
        }
    }
}

/// The colours the column is drawn with.
#[derive(Clone, Copy)]
pub(super) struct BlameColours {
    pub accent: gpui::Hsla,
    pub text: gpui::Hsla,
    pub muted: gpui::Hsla,
    pub border: gpui::Hsla,
}

/// One row of the column: shaded by age, labelled on the first line of a run (`above` is the
/// line on the row above, if any), with the commit's details on hover.
pub(super) fn cell(
    view: Option<&Arc<BlameView>>,
    row: usize,
    line: usize,
    above: Option<usize>,
    width: f32,
    colours: BlameColours,
    now: i64,
) -> gpui::Stateful<Div> {
    let base = div()
        .id(("blame", row))
        .flex_none()
        .w(px(width))
        .h(px(LINE_HEIGHT))
        .px(px(PAD))
        .flex()
        .justify_between()
        .border_r_1()
        .border_color(colours.border);
    let Some(view) = view else {
        return base;
    };
    let Some(heat) = view.heat_of(line) else {
        return base;
    };
    let label = view
        .starts_run(above, line)
        .then(|| view.label(line, now))
        .flatten();
    let text = if heat >= 0.5 {
        colours.text
    } else {
        colours.muted
    };
    let view = view.clone();
    base.bg(gpui::Hsla {
        a: 0.03 + 0.25 * heat,
        ..colours.accent
    })
    .text_color(text)
    .when_some(label, |d, (who, when)| {
        d.child(div().child(who)).child(div().child(when))
    })
    .tooltip(move |window, cx| {
        let text = view.details(line, now).unwrap_or_default();
        tip(text)(window, cx)
    })
}

/// Seconds since the Unix epoch, for ages.
pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

actions!(chorong, [ToggleBlame]);

pub(super) const COMMANDS: &[Command] =
    &[c("chorong::ToggleBlame", "Toggle Blame", Layer::View, 125)];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    on!(d, cx, ToggleBlame, |this, _w, cx| this.toggle_blame(cx))
}
