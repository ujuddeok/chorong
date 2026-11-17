//! Selecting and copying code text in a code pane: press, drag (with auto-scroll past the
//! list's top or bottom), double and triple click, Shift-click, Select All, Copy and Copy with
//! Location. The range maths is `crate::selection`; this file wires it to the pointer, the
//! keys and the clipboard.

use super::editor_state::SelectDrag;
use super::*;
use crate::selection::{self, Selection, Unit};
use gpui::{Bounds, DispatchPhase, MouseUpEvent, Pixels, Point, TextLayout};

/// Pointer to byte on a code line, the same mapping a click uses: the text layout gives a
/// display offset, and the line's tab map turns it into a file byte. Left of the text (the
/// gutter) is the line's start; `in_text` says which.
pub(super) fn hit_byte(
    layout: &TextLayout,
    position: Point<Pixels>,
    line: &crate::document::DisplayLine,
) -> (usize, bool) {
    if position.x >= layout.bounds().left() {
        let (Ok(i) | Err(i)) = layout.index_for_position(position);
        (line.byte_at(i), true)
    } else {
        (line.bytes.start, false)
    }
}

/// How often the list auto-scrolls while a drag is held past its top or bottom.
const AUTOSCROLL_TICK: std::time::Duration = std::time::Duration::from_millis(16);

impl Chorong {
    /// A press in the code (after the caret moved there): starts a selection of `clicks`'s
    /// unit, or with Shift extends the one before (or one from the old caret, `from`).
    pub(super) fn press_select(
        &mut self,
        byte: usize,
        in_text: bool,
        clicks: usize,
        extend: Option<(Option<Selection>, Option<usize>)>,
    ) {
        let ed = self.ed_mut();
        let Some(doc) = &ed.doc else { return };
        let epoch = ed.epoch;
        let sel = match extend {
            Some((Some(mut sel), _)) => {
                sel.extend(&doc.buffer, byte);
                sel
            }
            Some((None, Some(from))) => {
                let mut sel = Selection::new(&doc.buffer, from, Unit::Char, epoch);
                sel.extend(&doc.buffer, byte);
                sel
            }
            _ => {
                let unit = match clicks {
                    0 | 1 => Unit::Char,
                    2 if in_text => Unit::Word,
                    2 => Unit::Char,
                    _ => Unit::Line,
                };
                Selection::new(&doc.buffer, byte, unit, epoch)
            }
        };
        ed.selection = Some(sel);
        ed.drag = Some(SelectDrag::default());
    }

    /// The pointer moved over a code line during a drag: the head follows it.
    pub(super) fn drag_to(&mut self, pane: PaneId, byte: usize, cx: &mut Context<Self>) {
        let Some(ed) = self.editors.get_mut(&pane) else {
            return;
        };
        let (Some(doc), Some(sel), Some(_)) = (&ed.doc, &mut ed.selection, &ed.drag) else {
            return;
        };
        let byte = doc.buffer.clamp(byte);
        if sel.head != byte {
            sel.extend(&doc.buffer, byte);
            self.dirty_pane = Some(pane);
            cx.notify();
        }
    }

    /// The pointer moved over fold `fold`'s placeholder during a drag: its hidden lines are
    /// selected whole.
    pub(super) fn drag_over_fold(&mut self, pane: PaneId, fold: usize, cx: &mut Context<Self>) {
        let ed = self.ed_of(pane);
        let (Some(doc), Some(folds), Some(sel)) = (&ed.doc, &ed.folds, &ed.selection) else {
            return;
        };
        let Some(f) = folds.folds().get(fold) else {
            return;
        };
        let byte = selection::head_over_fold(&doc.buffer, &sel.anchor, f.start, f.end);
        self.drag_to(pane, byte, cx);
    }

    /// The head for row `row` reached from the side of the list: the start of its line from
    /// the left or the top, the end from the right or the bottom; over a fold's placeholder,
    /// past its hidden lines.
    fn head_at_row(&self, pane: PaneId, row: usize, start: bool) -> Option<usize> {
        let ed = self.ed_of(pane);
        let (doc, folds, sel) = (ed.doc.as_ref()?, ed.folds.as_ref()?, ed.selection.as_ref()?);
        let row = row.min(folds.rows().len().checked_sub(1)?);
        match folds.rows()[row] {
            Row::Placeholder(f) => {
                let f = folds.folds().get(f)?;
                Some(selection::head_over_fold(
                    &doc.buffer,
                    &sel.anchor,
                    f.start,
                    f.end,
                ))
            }
            Row::Line(_) | Row::Note(_) => {
                let r = doc.buffer.line_range(folds.line_at_row(row))?;
                Some(if start { r.start } else { r.end })
            }
        }
    }

    /// A pointer move anywhere in the window while a drag is under way (`bounds` is the code
    /// list's box). Over the code itself the line under the pointer has already moved the head;
    /// beside it the head goes to that row's start or end; above or below it the list
    /// auto-scrolls. A move without the button ends the drag.
    fn drag_pointer(
        &mut self,
        pane: PaneId,
        position: Point<Pixels>,
        pressed: bool,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !pressed {
            self.end_drag(pane, cx);
            return;
        }
        let Some(vp) = self.ed_of(pane).viewport() else {
            return;
        };
        let (top, bottom) = (bounds.top(), bounds.bottom());
        let beyond = if position.y < top {
            f32::from(position.y - top)
        } else if position.y > bottom {
            f32::from(position.y - bottom)
        } else {
            0.
        };
        if beyond == 0. {
            if let Some(d) = self.editors.get_mut(&pane).and_then(|e| e.drag.as_mut()) {
                d.beyond = 0.;
            }
            let (left, right) = (position.x < bounds.left(), position.x > bounds.right());
            if left || right {
                let row = ((f32::from(position.y - top) + vp.top) / vp.row_h).floor() as usize;
                if let Some(byte) = self.head_at_row(pane, row, left) {
                    self.drag_to(pane, byte, cx);
                }
            }
            return;
        }
        let start = match self.editors.get_mut(&pane).and_then(|e| e.drag.as_mut()) {
            Some(d) => {
                d.beyond = beyond;
                !std::mem::replace(&mut d.scrolling, true)
            }
            None => false,
        };
        if start {
            self.autoscroll(pane, cx);
        }
    }

    /// Scrolls the list while the drag stays past its top or bottom, faster the further out
    /// the pointer is, moving the head to the edge row as it goes.
    fn autoscroll(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSCROLL_TICK).await;
                let go_on = this
                    .update(cx, |this, cx| this.autoscroll_step(pane, cx))
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
        })
        .detach();
    }

    /// One step of `autoscroll`. False when the loop should stop.
    fn autoscroll_step(&mut self, pane: PaneId, cx: &mut Context<Self>) -> bool {
        let Some(ed) = self.editors.get_mut(&pane) else {
            return false;
        };
        let Some(drag) = ed.drag.as_mut() else {
            return false;
        };
        if drag.beyond == 0. {
            drag.scrolling = false;
            return false;
        }
        let beyond = drag.beyond;
        let (Some(vp), Some(rows)) = (ed.viewport(), ed.folds.as_ref().map(|f| f.rows().len()))
        else {
            return true;
        };
        let max = (rows as f32 * vp.row_h - vp.height).max(0.);
        let step = (beyond.abs() / 2.).clamp(4., 3. * vp.row_h) * beyond.signum();
        let new_top = (vp.top + step).clamp(0., max);
        {
            let state = ed.source_scroll.0.borrow();
            let x = state.base_handle.offset().x;
            state.base_handle.set_offset(gpui::point(x, px(-new_top)));
        }
        let row = if beyond < 0. {
            (new_top / vp.row_h).floor() as usize
        } else {
            ((new_top + vp.height - 1.) / vp.row_h).floor() as usize
        };
        if let Some(byte) = self.head_at_row(pane, row, beyond < 0.) {
            self.drag_to(pane, byte, cx);
        }
        self.dirty_pane = Some(pane);
        cx.notify();
        true
    }

    /// The button came up: the drag ends and the caret goes to the selection's head.
    pub(super) fn end_drag(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let Some(ed) = self.editors.get_mut(&pane) else {
            return;
        };
        if ed.drag.take().is_none() {
            return;
        }
        let moved = match (ed.selection_range(), &ed.selection) {
            (Some(_), Some(sel)) if ed.caret != Some(sel.head) => {
                ed.caret = Some(sel.head);
                true
            }
            _ => false,
        };
        if moved && pane == self.active_editor {
            self.refresh_context(cx);
        }
        self.dirty_pane = Some(pane);
        cx.notify();
    }

    /// While a drag is under way: an element over the code list that follows the pointer
    /// wherever it goes (past the list's edges, out of the pane) and sees the button come up.
    pub(super) fn drag_listener(&self, pane: PaneId, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let w = weak.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let pressed = ev.pressed_button == Some(MouseButton::Left);
                    w.update(cx, |this, cx| {
                        this.drag_pointer(pane, ev.position, pressed, bounds, cx)
                    })
                    .ok();
                });
                let w = weak.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                        w.update(cx, |this, cx| this.end_drag(pane, cx)).ok();
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// Esc's first job: drop a selection that selects something. Whether there was one.
    pub(super) fn clear_selection(&mut self) -> bool {
        let ed = self.ed_mut();
        let had = ed.selection_range().is_some();
        ed.selection = None;
        ed.drag = None;
        if had {
            self.touch_pane();
        }
        had
    }

    /// Select All: the whole file, folded lines included.
    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        let ed = self.ed_mut();
        let Some(doc) = &ed.doc else { return };
        let len = doc.buffer.len();
        ed.selection = Some(Selection {
            anchor: 0..0,
            head: len,
            unit: Unit::Char,
            epoch: ed.epoch,
        });
        ed.drag = None;
        self.touch_pane();
        cx.notify();
    }

    /// Copy (`located` false): the selection, or the caret's line. Copy with Location: the
    /// same text in a fenced block under a `path:first-last` header.
    pub(super) fn copy_selection(&mut self, located: bool, cx: &mut Context<Self>) {
        let ed = self.ed();
        let Some(doc) = &ed.doc else { return };
        let sel = ed.selection_range();
        let Some(caret) = ed.caret.or(sel.as_ref().map(|r| r.start)) else {
            return;
        };
        let range = selection::copy_range(&doc.buffer, sel.as_ref(), caret);
        let text = &doc.buffer.text()[range.clone()];
        let (first, last) = selection::line_span(&doc.buffer, &range);
        let out = if located {
            let tag = selection::fence_tag(doc.language, &doc.display_path);
            selection::with_location(&doc.display_path, (first, last), &tag, text)
        } else {
            text.to_string()
        };
        let lines = last + 1 - first;
        let what = match (sel.is_some(), lines) {
            (false, _) => "the line".to_string(),
            (true, 1) => format!("{} characters", text.chars().count()),
            (true, n) => format!("{n} lines"),
        };
        let with = if located { " with location" } else { "" };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(out));
        self.notify_text(format!("Copied {what}{with}"), cx);
    }
}
