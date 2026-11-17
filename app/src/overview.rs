//! The overview lane: a thin strip at the right edge of a code pane that has highlight groups
//! or is comparing (with neither it has nothing to point at). It maps the whole file to its
//! height and never scrolls. It shows where the highlight groups' occurrences and the
//! comparison's changed lines are, the part on screen as a faint band, and the caret's row.
//! A click centres the code on that place.
//!
//! What it draws is computed once per document, fold state, comparison gutter and highlight
//! groups ([`Marks`](crate::marks::Marks) revision) into a [`Shape`] of rows, and kept in the
//! pane's [`Overview`]. A frame maps those rows to pixels and paints one quad per pixel row
//! and colour (marks that land on the same pixel are drawn once), so its cost is bounded by
//! the lane's height, however many occurrences there are. [`Shape`] is pure and tested
//! without a window.

use crate::features::Probe;
use crate::minimap::{geometry, set_top};
use gpui::{
    Bounds, DispatchPhase, Hitbox, HitboxBehavior, Hsla, IntoElement, MouseButton, MouseDownEvent,
    Pixels, Styled, UniformListScrollHandle, Window, canvas, fill, point, px, size,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Width of the lane.
pub const WIDTH: f32 = 10.;
/// The change marks' column at the lane's left edge; highlight marks take the rest.
const CHANGE_W: f32 = 3.;
/// The least height a mark is drawn, however long the file.
const MARK_H: f32 = 2.;

/// Rows with marks, built once per key and painted every frame.
#[derive(Debug, Default, PartialEq)]
pub struct Shape {
    pub rows: usize,
    /// (row, palette entry), sorted and without repeats.
    marks: Vec<(u32, u16)>,
    pub palette: Vec<Hsla>,
    /// Rows a comparison marks changed, ascending, with whether the mark is hollow.
    changes: Vec<(u32, bool)>,
}

impl Shape {
    /// `marks` are (row, colour) pairs in any order; `changes` are (row, hollow), ascending.
    pub fn build(
        rows: usize,
        marks: impl IntoIterator<Item = (usize, Hsla)>,
        changes: Vec<(u32, bool)>,
    ) -> Shape {
        let mut palette: Vec<Hsla> = Vec::new();
        let mut last: Option<(Hsla, u16)> = None;
        let mut out: Vec<(u32, u16)> = marks
            .into_iter()
            .filter(|(row, _)| *row < rows)
            .map(|(row, c)| {
                let i = match last {
                    Some((h, i)) if h == c => i,
                    _ => {
                        let i = match palette.iter().position(|p| *p == c) {
                            Some(i) => i as u16,
                            None => {
                                palette.push(c);
                                (palette.len() - 1) as u16
                            }
                        };
                        last = Some((c, i));
                        i
                    }
                };
                (row as u32, i)
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        Shape {
            rows,
            marks: out,
            palette,
            changes,
        }
    }

    /// The highlight marks as spans in a lane `height` tall, each at least `min_h`:
    /// (top, height, palette entry). Marks of one colour that touch or overlap are one span,
    /// so the quads to paint are bounded by the lane's height, and none of one colour overlap
    /// (overlapping quads are what made GPUI's paint slow: about 3 ms a frame for 1,000
    /// separately drawn marks, measured).
    pub fn mark_spans(&self, height: f32, min_h: f32) -> Vec<(f32, f32, u16)> {
        let mut open: Vec<Option<(f32, f32)>> = vec![None; self.palette.len()];
        let mut out = Vec::new();
        for &(row, c) in &self.marks {
            let (y, end) = self.place(row, height, min_h);
            match &mut open[c as usize] {
                Some((_, e)) if y <= *e + 0.5 => *e = e.max(end),
                slot => {
                    if let Some((t, e)) = slot.replace((y, end)) {
                        out.push((t, e - t, c));
                    }
                }
            }
        }
        for (c, slot) in open.into_iter().enumerate() {
            if let Some((t, e)) = slot {
                out.push((t, e - t, c as u16));
            }
        }
        out
    }

    /// The change marks as spans, merged the same way: (top, height, hollow).
    pub fn change_spans(&self, height: f32, min_h: f32) -> Vec<(f32, f32, bool)> {
        let mut open: Option<(f32, f32, bool)> = None;
        let mut out = Vec::new();
        for &(row, hollow) in &self.changes {
            let (y, end) = self.place(row, height, min_h);
            match &mut open {
                Some((_, e, h)) if *h == hollow && y <= *e + 0.5 => *e = e.max(end),
                slot => {
                    if let Some((t, e, h)) = slot.replace((y, end, hollow)) {
                        out.push((t, e - t, h));
                    }
                }
            }
        }
        out.extend(open.map(|(t, e, h)| (t, e - t, h)));
        out
    }

    /// Where row `row`'s mark starts and ends: a row's height, at least `min_h`, kept inside
    /// the lane.
    fn place(&self, row: u32, height: f32, min_h: f32) -> (f32, f32) {
        let h = (height / self.rows.max(1) as f32).max(min_h);
        let y = self.y(row, height).min(height - h).max(0.);
        (y, y + h)
    }

    /// Where row `row` starts in a lane `height` tall.
    pub fn y(&self, row: u32, height: f32) -> f32 {
        if self.rows == 0 {
            return 0.;
        }
        row as f32 / self.rows as f32 * height
    }

    /// The row at `y` in a lane `height` tall.
    pub fn row_at(&self, y: f32, height: f32) -> usize {
        if height <= 0. || self.rows == 0 {
            return 0;
        }
        ((y / height).clamp(0., 1.) * self.rows as f32).min(self.rows as f32 - 1.) as usize
    }
}

/// What the cached shape was built from: change any part and it is rebuilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub doc: usize,
    pub epoch: u64,
    pub folds: u64,
    pub gutter: usize,
    /// `Marks::revision`.
    pub marks: u64,
    /// The number of marker colours.
    pub palette: usize,
}

/// One code pane's lane: its cached shape. Clones share it.
#[derive(Clone, Default)]
pub struct Overview(Rc<RefCell<Option<Cached>>>);

type Cached = (Key, Arc<Shape>);

/// Colours for [`Overview::element`].
#[derive(Clone, Copy)]
pub struct Colours {
    pub background: Hsla,
    pub border: Hsla,
    pub band: Hsla,
    pub caret: Hsla,
    pub change: Hsla,
}

impl Overview {
    /// The shape for `key`, built by `build` only when the key differs from last time.
    pub fn shape(&self, key: Key, build: impl FnOnce() -> Shape) -> Arc<Shape> {
        let mut st = self.0.borrow_mut();
        match &*st {
            Some((k, shape)) if *k == key => shape.clone(),
            _ => {
                let shape = Arc::new(build());
                *st = Some((key, shape.clone()));
                shape
            }
        }
    }

    /// Drops the cache (the lane is not drawn).
    pub fn hide(&self) {
        self.0.borrow_mut().take();
    }

    /// The lane for a code pane scrolled by `scroll`, with the caret on `caret_row`. `probe`
    /// times each paint.
    pub fn element(
        &self,
        shape: Arc<Shape>,
        caret_row: Option<usize>,
        scroll: UniformListScrollHandle,
        colours: Colours,
        probe: Probe,
    ) -> impl IntoElement {
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                {
                    let _t = probe.start();
                    paint(&shape, caret_row, &scroll, colours, bounds, window);
                }
                listen(shape, scroll, bounds, hitbox, window);
            },
        )
        .flex_none()
        .w(px(WIDTH))
        .h_full()
    }
}

fn paint(
    shape: &Shape,
    caret_row: Option<usize>,
    scroll: &UniformListScrollHandle,
    c: Colours,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let (x0, y0) = (bounds.origin.x, bounds.origin.y);
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    window.paint_quad(fill(bounds, c.background));
    window.paint_quad(fill(
        Bounds::new(bounds.origin, size(px(1.), bounds.size.height)),
        c.border,
    ));
    // The part on screen.
    let g = geometry(shape.rows, scroll, h);
    let content = g.max + g.view_h;
    if content > 0. {
        let top = g.top / content * h;
        let band = (g.view_h / content * h).max(MARK_H);
        window.paint_quad(fill(
            Bounds::new(point(x0 + px(1.), y0 + px(top)), size(px(w - 1.), px(band))),
            c.band,
        ));
    }
    for (y, span, hollow) in shape.change_spans(h, MARK_H) {
        let colour = if hollow {
            c.change.opacity(0.45)
        } else {
            c.change
        };
        window.paint_quad(fill(
            Bounds::new(point(x0 + px(1.), y0 + px(y)), size(px(CHANGE_W), px(span))),
            colour,
        ));
    }
    let left = 1. + CHANGE_W + 1.;
    for (y, span, colour) in shape.mark_spans(h, MARK_H) {
        window.paint_quad(fill(
            Bounds::new(
                point(x0 + px(left), y0 + px(y)),
                size(px(w - left), px(span)),
            ),
            shape.palette[colour as usize],
        ));
    }
    if let Some(row) = caret_row {
        let y = shape.y(row as u32, h);
        window.paint_quad(fill(
            Bounds::new(point(x0 + px(1.), y0 + px(y)), size(px(w - 1.), px(MARK_H))),
            c.caret,
        ));
    }
}

/// A click centres the code on the row at the pointer.
fn listen(
    shape: Arc<Shape>,
    scroll: UniformListScrollHandle,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    window: &mut Window,
) {
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, _| {
        if phase != DispatchPhase::Bubble
            || ev.button != MouseButton::Left
            || !hitbox.is_hovered(window)
        {
            return;
        }
        let h = f32::from(bounds.size.height);
        let row = shape.row_at(f32::from(ev.position.y - bounds.origin.y), h);
        let g = geometry(shape.rows, &scroll, h);
        let top = (row as f32 * g.item_h + g.item_h / 2. - g.view_h / 2.).clamp(0., g.max);
        set_top(&scroll, top);
        window.refresh();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colour(h: f32) -> Hsla {
        gpui::hsla(h, 0.5, 0.5, 1.)
    }

    #[test]
    fn marks_are_sorted_deduplicated_and_kept_in_range() {
        let s = Shape::build(
            10,
            [
                (5, colour(0.1)),
                (2, colour(0.2)),
                (5, colour(0.1)),
                (12, colour(0.1)),
            ],
            vec![],
        );
        assert_eq!(s.marks, [(2, 1), (5, 0)]);
        assert_eq!(s.palette, [colour(0.1), colour(0.2)]);
    }

    #[test]
    fn touching_marks_of_one_colour_are_one_span() {
        // 10,000 rows in a 500 px lane: 20 rows per pixel. A mark on every row is one span,
        // whatever the count; another colour on the first row is a span of its own.
        let marks = (0..10_000).map(|r| (r, colour(0.1)));
        let s = Shape::build(10_000, marks.chain([(0, colour(0.2))]), vec![]);
        assert_eq!(s.mark_spans(500., 2.), [(0., 500.0, 0), (0., 2., 1)]);
        // Every 10th row (1,000 marks, half a pixel apart): still one span.
        let s = Shape::build(
            10_000,
            (0..10_000).step_by(10).map(|r| (r, colour(0.1))),
            vec![],
        );
        assert_eq!(s.mark_spans(500., 2.).len(), 1);
        // Far apart marks keep their own places; a short file's rows are taller than a mark.
        let s = Shape::build(
            100,
            [(10, colour(0.1)), (11, colour(0.1)), (50, colour(0.1))],
            vec![],
        );
        assert_eq!(s.mark_spans(500., 2.), [(50., 10., 0), (250., 5., 0)]);
    }

    #[test]
    fn changes_merge_by_kind_when_they_touch() {
        let s = Shape::build(
            1000,
            [],
            vec![(0, false), (1, false), (2, true), (500, false)],
        );
        let rounded: Vec<_> = s
            .change_spans(100., 2.)
            .into_iter()
            .map(|(y, h, hollow)| ((y * 100.).round(), (h * 100.).round(), hollow))
            .collect();
        assert_eq!(
            rounded,
            [(0., 210., false), (20., 200., true), (5000., 200., false)]
        );
    }

    #[test]
    fn pointer_maps_to_rows_across_the_whole_file() {
        let s = Shape::build(10_000, [], vec![]);
        assert_eq!(s.row_at(0., 500.), 0);
        assert_eq!(s.row_at(250., 500.), 5000);
        assert_eq!(s.row_at(500., 500.), 9999);
        assert_eq!(s.row_at(-5., 500.), 0);
        assert_eq!(Shape::build(0, [], vec![]).row_at(10., 500.), 0);
    }

    #[test]
    fn the_shape_is_built_once_per_key() {
        let lane = Overview::default();
        let key = Key {
            doc: 1,
            epoch: 1,
            folds: 1,
            gutter: 0,
            marks: 7,
            palette: 8,
        };
        let mut built = 0;
        let mut get = |k: Key| {
            lane.shape(k, || {
                built += 1;
                Shape::build(1, [], vec![])
            })
        };
        let a = get(key);
        assert!(Arc::ptr_eq(&a, &get(key)));
        get(Key { marks: 8, ..key });
        assert_eq!(built, 2);
    }
}
