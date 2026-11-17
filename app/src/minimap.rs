//! The minimap: a narrow strip at a code pane's right edge drawing every visible row of the
//! file in miniature (2 px per row, one pixel per display column), with a frame over the part
//! on screen. Each row is a few blocks: characters at most one blank apart form a block, drawn
//! in the syntax colour most of its characters have, so a comment, a string or a keyword-led
//! line keeps its colour and a frame paints a handful of quads per row rather than one per
//! token (a sixth of the cost, measured). A click centres the code on the row under
//! the pointer; dragging scrolls; the wheel over it scrolls the code.
//!
//! The file's shape ([`Shape`]) is computed once per document, fold state, comparison gutter
//! and colours, and kept in the pane's [`Minimap`]; a frame only paints the rows of it that
//! fit in the strip and moves the frame, so scrolling costs the same however long the file.
//! [`Geometry`] (where the strip and frame sit for a scroll position, and the reverse for the
//! pointer) is pure and tested without a window.
//!
//! Marks: comparison changes are part of the shape (a bar at the strip's left edge, like the
//! gutter's). Highlight groups are drawn on top as [`Tick`]s, placed once per change of the
//! groups (`Marks::revision`) and folds and kept sorted by row, so a frame only draws those in
//! view: placing 10,000 occurrences every frame cost about 4 ms (measured).

use crate::features::Probe;
use gpui::{
    App, Bounds, DispatchPhase, HighlightStyle, Hitbox, HitboxBehavior, Hsla, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollWheelEvent, Styled,
    UniformListScrollHandle, Window, canvas, fill, point, px, size,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// Height of one row in the strip.
pub const ROW_PX: f32 = 2.;
/// Width of one display column in the strip.
pub const COL_PX: f32 = 1.;
/// Width of the strip.
pub const WIDTH: f32 = 100.;
/// Room at the strip's left edge for the change bar.
const PAD_LEFT: f32 = 6.;
const CHANGE_BAR: f32 = 3.;
/// Columns beyond this are not drawn.
pub const MAX_COLS: usize = ((WIDTH - PAD_LEFT - 2.) / COL_PX) as usize;
/// A code pane at least this wide shows the minimap when `editor.minimap_always` is on.
pub const MIN_PANE_WIDTH: f32 = 640.;
/// Where a code pane stands on a focus page, for whether its minimap shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Not in a focus layout, or sharing the page with other panes.
    No,
    /// The only pane on the page (maximised): the minimap shows.
    Alone,
    /// One of exactly two panes on the page, both code panes: the minimap shows if the pane
    /// is at least [`MIN_PANE_WIDTH`] wide.
    Pair,
}

impl Focus {
    /// `pane`'s place among the panes on screen in a focus layout (`None` outside one);
    /// `is_code` says which are code panes.
    pub fn of<T: PartialEq>(
        on_screen: Option<&[T]>,
        pane: &T,
        is_code: impl Fn(&T) -> bool,
    ) -> Focus {
        match on_screen {
            Some([only]) if only == pane => Focus::Alone,
            Some(two @ [_, _]) if two.contains(pane) && two.iter().all(is_code) => Focus::Pair,
            _ => Focus::No,
        }
    }
}

/// How strongly the code's colours are drawn: the strip is a shape, not text to read.
const ALPHA: f32 = 0.7;

/// A block of one colour on one row: `len` columns from `col`, palette entry `colour`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub col: u16,
    pub len: u16,
    pub colour: u16,
}

/// What one row of the code pane shows, for [`Shape::build`].
pub enum RowSource<'a> {
    /// A line: its display text (tabs expanded), syntax highlights (byte ranges into that
    /// text) and whether a comparison marks it changed (`Some(hollow)`).
    Line {
        text: &'a str,
        highlights: &'a [(Range<usize>, HighlightStyle)],
        change: Option<bool>,
    },
    /// A folded body's `⋯ N lines` placeholder.
    Placeholder,
}

/// The miniature of a file: runs per row, built once and painted every frame.
#[derive(Debug, Default, PartialEq)]
pub struct Shape {
    /// Row `r`'s runs are `runs[starts[r]..starts[r + 1]]`.
    starts: Vec<u32>,
    runs: Vec<Run>,
    pub palette: Vec<Hsla>,
    /// Rows a comparison marks changed, ascending, with whether the mark is hollow.
    changes: Vec<(u32, bool)>,
}

impl Shape {
    /// Runs of non-blank characters of one colour, cut at [`MAX_COLS`]. Text in no highlight
    /// takes `default`; a placeholder is one short block in `muted`.
    pub fn build<'a>(
        rows: impl IntoIterator<Item = RowSource<'a>>,
        default: Hsla,
        muted: Hsla,
    ) -> Shape {
        let mut shape = Shape {
            starts: vec![0],
            ..Shape::default()
        };
        for (row, source) in rows.into_iter().enumerate() {
            match source {
                RowSource::Line {
                    text,
                    highlights,
                    change,
                } => {
                    shape.line(text, highlights, default);
                    if let Some(hollow) = change {
                        shape.changes.push((row as u32, hollow));
                    }
                }
                RowSource::Placeholder => {
                    let colour = shape.colour(muted);
                    shape.runs.push(Run {
                        col: 2,
                        len: 8,
                        colour,
                    });
                }
            }
            shape.starts.push(shape.runs.len() as u32);
        }
        shape
    }

    fn line(&mut self, text: &str, highlights: &[(Range<usize>, HighlightStyle)], default: Hsla) {
        let mut spans = highlights.iter().peekable();
        // The block being built: first and one-past-last column, and columns per colour.
        let mut open: Option<(usize, usize)> = None;
        let mut counts: Vec<(u16, u16)> = Vec::new();
        // The palette entry of the colour looked up last: most neighbours share it.
        let mut last: Option<(Hsla, u16)> = None;
        for (col, (byte, ch)) in text.char_indices().enumerate() {
            if col >= MAX_COLS {
                break;
            }
            if ch.is_whitespace() {
                continue;
            }
            while spans.peek().is_some_and(|(r, _)| r.end <= byte) {
                spans.next();
            }
            let hsla = spans
                .peek()
                .filter(|(r, _)| r.start <= byte)
                .and_then(|(_, s)| s.color)
                .unwrap_or(default);
            let colour = match last {
                Some((h, i)) if h == hsla => i,
                _ => {
                    let i = self.colour(hsla);
                    last = Some((hsla, i));
                    i
                }
            };
            match &mut open {
                // At most one blank since the block's last character: the same block.
                Some((_, end)) if col <= *end + 1 => *end = col + 1,
                _ => {
                    self.close(open.take(), &mut counts);
                    open = Some((col, col + 1));
                }
            }
            match counts.iter_mut().find(|(c, _)| *c == colour) {
                Some((_, n)) => *n += 1,
                None => counts.push((colour, 1)),
            }
        }
        self.close(open, &mut counts);
    }

    /// Ends a block: one run in the colour most of its characters have (the first of a tie).
    fn close(&mut self, block: Option<(usize, usize)>, counts: &mut Vec<(u16, u16)>) {
        if let Some((start, end)) = block {
            let mut best = counts[0];
            for &c in &counts[1..] {
                if c.1 > best.1 {
                    best = c;
                }
            }
            self.runs.push(Run {
                col: start as u16,
                len: (end - start) as u16,
                colour: best.0,
            });
        }
        counts.clear();
    }

    fn colour(&mut self, c: Hsla) -> u16 {
        let c = c.opacity(ALPHA);
        match self.palette.iter().position(|p| *p == c) {
            Some(i) => i as u16,
            None => {
                self.palette.push(c);
                (self.palette.len() - 1) as u16
            }
        }
    }

    pub fn rows(&self) -> usize {
        self.starts.len() - 1
    }

    pub fn row(&self, row: usize) -> &[Run] {
        match (self.starts.get(row), self.starts.get(row + 1)) {
            (Some(&a), Some(&b)) => &self.runs[a as usize..b as usize],
            _ => &[],
        }
    }

    /// The change marks on `rows`.
    pub fn changes_in(&self, rows: Range<usize>) -> &[(u32, bool)] {
        let from = self
            .changes
            .partition_point(|(r, _)| (*r as usize) < rows.start);
        let to = self
            .changes
            .partition_point(|(r, _)| (*r as usize) < rows.end);
        &self.changes[from..to]
    }

    /// Heap bytes held, for the cost notes.
    #[cfg(test)]
    pub fn approx_bytes(&self) -> usize {
        self.starts.capacity() * 4
            + self.runs.capacity() * size_of::<Run>()
            + self.palette.capacity() * size_of::<Hsla>()
            + self.changes.capacity() * 8
    }
}

/// Where the strip and its frame sit for one scroll position of the code pane, all in
/// pixels: the code pane is `view_h` tall, scrolled `top` of `max`, with rows `item_h` tall;
/// the strip is `strip_h` tall and draws `rows` rows.
///
/// When the miniature is taller than the strip it scrolls too, in proportion, so the top of
/// the file shows at the top of the strip and the end at the bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub rows: usize,
    pub strip_h: f32,
    pub view_h: f32,
    pub item_h: f32,
    pub top: f32,
    pub max: f32,
}

impl Geometry {
    fn content_h(&self) -> f32 {
        self.rows as f32 * ROW_PX
    }

    fn scale(&self) -> f32 {
        if self.item_h > 0. {
            ROW_PX / self.item_h
        } else {
            0.
        }
    }

    /// How far the miniature is scrolled within the strip.
    pub fn offset(&self) -> f32 {
        let over = self.content_h() - self.strip_h;
        if over <= 0. || self.max <= 0. {
            return 0.;
        }
        (self.top / self.max).clamp(0., 1.) * over
    }

    /// The frame's top (relative to the strip) and height.
    pub fn frame(&self) -> (f32, f32) {
        let s = self.scale();
        (self.top * s - self.offset(), self.view_h * s)
    }

    /// The rows that fall (even partly) inside the strip.
    pub fn visible_rows(&self) -> Range<usize> {
        let off = self.offset();
        let first = (off / ROW_PX).floor().max(0.) as usize;
        let last = ((off + self.strip_h) / ROW_PX).ceil().max(0.) as usize;
        first.min(self.rows)..last.min(self.rows)
    }

    /// The scroll top that centres the code pane on the row at `y` in the strip.
    pub fn top_centred_at(&self, y: f32) -> f32 {
        let row = ((y + self.offset()) / ROW_PX).clamp(0., self.rows as f32);
        (row * self.item_h - self.view_h / 2.).clamp(0., self.max.max(0.))
    }

    /// The scroll top after dragging the frame by `dy` from where it was at scroll top
    /// `from`: the frame follows the pointer.
    pub fn top_dragged(&self, from: f32, dy: f32) -> f32 {
        // How far the frame moves per pixel of scroll.
        let k = if self.content_h() <= self.strip_h || self.max <= 0. {
            self.scale()
        } else {
            (self.strip_h - self.view_h * self.scale()) / self.max
        };
        if k <= 0. {
            return from;
        }
        (from + dy / k).clamp(0., self.max.max(0.))
    }

    /// Whether `y` (in the strip) is on the frame.
    pub fn on_frame(&self, y: f32) -> bool {
        let (top, h) = self.frame();
        (top..top + h.max(ROW_PX)).contains(&y)
    }
}

/// A highlight-group occurrence to draw on top of the shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    pub row: u32,
    pub col: u16,
    pub len: u16,
    pub colour: Hsla,
}

/// What the cached shape was built from: change any part and it is rebuilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    /// The document (its `Arc`'s address) and the pane's epoch.
    pub doc: usize,
    pub epoch: u64,
    /// `Folds::generation`.
    pub folds: u64,
    /// The comparison gutter (its `Arc`'s address, 0 for none).
    pub gutter: usize,
    /// The default and muted colours, as bits.
    pub colours: [u32; 2],
}

#[derive(Default)]
struct State {
    cache: Option<(Key, Arc<Shape>)>,
    /// The highlight ticks, and the key and `Marks::revision` they were placed for.
    ticks: Option<(Key, u64, Arc<Vec<Tick>>)>,
    /// A drag in progress: the pointer's y and the scroll top when it started.
    drag: Option<(f32, f32)>,
    /// Whether the strip was drawn last frame (the pane's list is narrower by its width).
    shown: bool,
}

/// One code pane's minimap: its cached shape and drag state. Clones share it.
#[derive(Clone, Default)]
pub struct Minimap(Rc<RefCell<State>>);

/// Colours for [`Minimap::element`].
#[derive(Clone, Copy)]
pub struct Colours {
    pub background: Hsla,
    pub border: Hsla,
    pub frame: Hsla,
    pub frame_border: Hsla,
    pub change: Hsla,
}

impl Minimap {
    /// The shape for `key`, built by `build` only when the key differs from last time.
    pub fn shape(&self, key: Key, build: impl FnOnce() -> Shape) -> Arc<Shape> {
        let mut st = self.0.borrow_mut();
        match &st.cache {
            Some((k, shape)) if *k == key => shape.clone(),
            _ => {
                let shape = Arc::new(build());
                st.cache = Some((key, shape.clone()));
                shape
            }
        }
    }

    /// The highlight ticks for `key` and highlight-group revision `marks`, placed by `place`
    /// (in any order) only when either differs from last time; sorted by row.
    pub fn ticks(&self, key: Key, marks: u64, place: impl FnOnce() -> Vec<Tick>) -> Arc<Vec<Tick>> {
        let mut st = self.0.borrow_mut();
        match &st.ticks {
            Some((k, m, ticks)) if *k == key && *m == marks => ticks.clone(),
            _ => {
                let mut ticks = place();
                ticks.sort_by_key(|t| (t.row, t.col));
                let ticks = Arc::new(ticks);
                st.ticks = Some((key, marks, ticks.clone()));
                ticks
            }
        }
    }

    /// Whether the strip should show in a pane whose list was laid out `list_w` wide last
    /// frame, when only width decides (`editor.minimap_always`).
    pub fn wide_enough(&self, list_w: f32) -> bool {
        let shown = self.0.borrow().shown;
        list_w + if shown { WIDTH } else { 0. } >= MIN_PANE_WIDTH
    }

    /// Records whether the strip is drawn this frame; a hidden strip drops its cache.
    pub fn set_shown(&self, shown: bool) {
        let mut st = self.0.borrow_mut();
        st.shown = shown;
        if !shown {
            st.cache = None;
            st.ticks = None;
            st.drag = None;
        }
    }

    /// The strip for a code pane scrolled by `scroll`. `probe` times each paint.
    pub fn element(
        &self,
        shape: Arc<Shape>,
        ticks: Arc<Vec<Tick>>,
        scroll: UniformListScrollHandle,
        colours: Colours,
        probe: Probe,
    ) -> impl IntoElement {
        let state = self.0.clone();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _cx| {
                {
                    let _t = probe.start();
                    paint(&shape, &ticks, &scroll, colours, bounds, window);
                }
                listen(state, shape.rows(), scroll, bounds, hitbox, window);
            },
        )
        .flex_none()
        .w(px(WIDTH))
        .h_full()
    }
}

/// Where a code pane of `rows` rows scrolled by `scroll` is, as last laid out, for a strip
/// `strip_h` tall. GPUI's `last_item_size` is the list's viewport (`item`) and its whole
/// content (`contents`), so a row is the content height over the row count.
/// Ticks (sorted by row) as quads to paint: each tick with the number of rows it covers.
/// Ticks of the same place and colour on consecutive rows are one quad, and each fits in its
/// row: overlapping quads are slow for GPUI to order (10,000 separately drawn 3 px ticks on
/// 2 px rows cost about 2.7 ms a frame, measured).
pub fn tick_spans(ticks: &[Tick]) -> Vec<(Tick, u32)> {
    let mut open: Vec<(Tick, u32)> = Vec::new();
    let mut out = Vec::new();
    for t in ticks {
        // Spans that ended before the row above this tick cannot grow any more.
        open.retain(|(s, n)| {
            let live = s.row + n >= t.row;
            if !live {
                out.push((*s, *n));
            }
            live
        });
        match open.iter_mut().find(|(s, n)| {
            s.row + n == t.row && (s.col, s.len, s.colour) == (t.col, t.len, t.colour)
        }) {
            Some((_, n)) => *n += 1,
            None => open.push((*t, 1)),
        }
    }
    out.extend(open);
    out
}

pub(crate) fn geometry(rows: usize, scroll: &UniformListScrollHandle, strip_h: f32) -> Geometry {
    let st = scroll.0.borrow();
    let base = &st.base_handle;
    let max = f32::from(base.max_offset().height).max(0.);
    let top = (-f32::from(base.offset().y)).clamp(0., max);
    let (view_h, item_h) = st.last_item_size.map_or((0., 0.), |s| {
        (
            f32::from(s.item.height),
            f32::from(s.contents.height) / rows.max(1) as f32,
        )
    });
    Geometry {
        rows,
        strip_h,
        view_h,
        item_h,
        top,
        max,
    }
}

pub(crate) fn set_top(scroll: &UniformListScrollHandle, top: f32) {
    let base = scroll.0.borrow().base_handle.clone();
    let x = base.offset().x;
    base.set_offset(point(x, px(-top)));
}

fn paint(
    shape: &Shape,
    ticks: &[Tick],
    scroll: &UniformListScrollHandle,
    c: Colours,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    window.paint_quad(fill(bounds, c.background));
    window.paint_quad(fill(
        Bounds::new(bounds.origin, size(px(1.), bounds.size.height)),
        c.border,
    ));
    let g = geometry(shape.rows(), scroll, f32::from(bounds.size.height));
    if g.item_h <= 0. {
        return;
    }
    let (x0, y0) = (
        f32::from(bounds.origin.x) + PAD_LEFT,
        f32::from(bounds.origin.y) - g.offset(),
    );
    let rows = g.visible_rows();
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        let block_h = ROW_PX * 0.75;
        for row in rows.clone() {
            let y = y0 + row as f32 * ROW_PX;
            for run in shape.row(row) {
                window.paint_quad(fill(
                    Bounds::new(
                        point(px(x0 + run.col as f32 * COL_PX), px(y)),
                        size(px(run.len as f32 * COL_PX), px(block_h)),
                    ),
                    shape.palette[run.colour as usize],
                ));
            }
        }
        for &(row, hollow) in shape.changes_in(rows.clone()) {
            let colour = if hollow {
                c.change.opacity(0.45)
            } else {
                c.change
            };
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x + px(1.5), px(y0 + row as f32 * ROW_PX)),
                    size(px(CHANGE_BAR), px(ROW_PX)),
                ),
                colour,
            ));
        }
        let from = ticks.partition_point(|t| (t.row as usize) < rows.start);
        let to = ticks.partition_point(|t| (t.row as usize) < rows.end);
        for (t, n) in tick_spans(&ticks[from..to]) {
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        px(x0 + t.col as f32 * COL_PX - 1.),
                        px(y0 + t.row as f32 * ROW_PX),
                    ),
                    size(
                        px((t.len as f32 * COL_PX + 2.).max(3.)),
                        px(n as f32 * ROW_PX),
                    ),
                ),
                t.colour,
            ));
        }
        let (top, h) = g.frame();
        let frame = Bounds::new(
            point(bounds.origin.x + px(1.), bounds.origin.y + px(top)),
            size(bounds.size.width - px(1.), px(h.max(ROW_PX))),
        );
        window.paint_quad(
            fill(frame, c.frame)
                .border_widths(px(1.))
                .border_color(c.frame_border),
        );
    });
}

/// Click to centre, drag to scroll, wheel to scroll. The listeners last one frame and are
/// registered again by the next paint.
fn listen(
    state: Rc<RefCell<State>>,
    rows: usize,
    scroll: UniformListScrollHandle,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    window: &mut Window,
) {
    let strip_h = f32::from(bounds.size.height);
    let local = move |y: Pixels| f32::from(y - bounds.origin.y);
    {
        let (state, scroll, hitbox) = (state.clone(), scroll.clone(), hitbox.clone());
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, _| {
            if phase != DispatchPhase::Bubble
                || ev.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let g = geometry(rows, &scroll, strip_h);
            let y = local(ev.position.y);
            let from = if g.on_frame(y) {
                g.top
            } else {
                let top = g.top_centred_at(y);
                set_top(&scroll, top);
                top
            };
            state.borrow_mut().drag = Some((y, from));
            window.refresh();
        });
    }
    {
        let (state, scroll) = (state.clone(), scroll.clone());
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, _| {
            let Some((y0, from)) = state.borrow().drag else {
                return;
            };
            if phase != DispatchPhase::Capture {
                return;
            }
            if ev.pressed_button != Some(MouseButton::Left) {
                state.borrow_mut().drag = None;
                return;
            }
            let g = geometry(rows, &scroll, strip_h);
            set_top(&scroll, g.top_dragged(from, local(ev.position.y) - y0));
            window.refresh();
        });
    }
    {
        let state = state.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, _| {
            if phase == DispatchPhase::Capture {
                state.borrow_mut().drag = None;
            }
        });
    }
    window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, window, cx: &mut App| {
        if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
            return;
        }
        let g = geometry(rows, &scroll, strip_h);
        let dy = f32::from(ev.delta.pixel_delta(px(g.item_h.max(1.))).y);
        set_top(&scroll, (g.top - dy).clamp(0., g.max));
        cx.stop_propagation();
        window.refresh();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colour(h: f32) -> Hsla {
        gpui::hsla(h, 0.5, 0.5, 1.)
    }

    fn style(h: f32) -> HighlightStyle {
        HighlightStyle {
            color: Some(colour(h)),
            ..Default::default()
        }
    }

    fn line<'a>(text: &'a str, hl: &'a [(Range<usize>, HighlightStyle)]) -> RowSource<'a> {
        RowSource::Line {
            text,
            highlights: hl,
            change: None,
        }
    }

    fn cols(shape: &Shape, row: usize) -> Vec<(u16, u16)> {
        shape.row(row).iter().map(|r| (r.col, r.len)).collect()
    }

    #[test]
    fn blocks_join_words_one_blank_apart_in_their_main_colour() {
        // `fn` keyword-coloured, the rest default: one block, mostly default.
        let kw = [(0..2, style(0.1))];
        // A comment after two blanks: a block of its own, in the comment colour.
        let text = "let x = 1;  // note";
        let comment = [(12..19, style(0.3))];
        let shape = Shape::build(
            [
                line("fn main() {", &kw),
                line(text, &comment),
                line("a  b", &[]),
            ],
            colour(0.5),
            colour(0.9),
        );
        assert_eq!(cols(&shape, 0), [(0, 11)]);
        let colour_of = |row: usize, i: usize| shape.palette[shape.row(row)[i].colour as usize];
        assert_eq!(colour_of(0, 0), colour(0.5).opacity(ALPHA));
        assert_eq!(cols(&shape, 1), [(0, 10), (12, 7)]);
        assert_eq!(colour_of(1, 1), colour(0.3).opacity(ALPHA));
        assert_eq!(cols(&shape, 2), [(0, 1), (3, 1)]);
    }

    #[test]
    fn columns_count_characters_and_a_tie_keeps_the_first_colour() {
        // Two-byte characters count one column each; `éé` default, `.ab` (bytes 4..7) coloured.
        let hl = [(4..7, style(0.2))];
        let shape = Shape::build(
            [line("éé.ab", &hl), line("ab.c", &[(2..4, style(0.2))])],
            colour(0.5),
            colour(0.9),
        );
        assert_eq!(cols(&shape, 0), [(0, 5)]);
        assert_eq!(
            shape.palette[shape.row(0)[0].colour as usize],
            colour(0.2).opacity(ALPHA)
        );
        assert_eq!(
            shape.palette[shape.row(1)[0].colour as usize],
            colour(0.5).opacity(ALPHA)
        );
    }

    #[test]
    fn long_lines_are_cut_and_blank_lines_are_empty() {
        let long = "x".repeat(MAX_COLS + 50);
        let shape = Shape::build(
            [
                line(&long, &[]),
                line("", &[]),
                line("    ", &[]),
                RowSource::Placeholder,
            ],
            colour(0.5),
            colour(0.9),
        );
        assert_eq!(shape.rows(), 4);
        assert_eq!(
            shape.row(0),
            [Run {
                col: 0,
                len: MAX_COLS as u16,
                colour: 0
            }]
        );
        assert!(shape.row(1).is_empty() && shape.row(2).is_empty());
        assert_eq!(shape.row(3).len(), 1);
        assert!(shape.row(4).is_empty(), "past the end");
    }

    #[test]
    fn changes_are_kept_by_row() {
        let rows = (0..10).map(|i| RowSource::Line {
            text: "x",
            highlights: &[],
            change: (i % 3 == 0).then_some(i == 6),
        });
        let shape = Shape::build(rows, colour(0.5), colour(0.9));
        assert_eq!(
            shape.changes_in(0..10),
            [(0, false), (3, false), (6, true), (9, false)]
        );
        assert_eq!(shape.changes_in(4..9), [(6, true)]);
    }

    fn big(top: f32) -> Geometry {
        // 10,000 rows of 20 px in a 1,000 px pane; the strip is 1,000 px (500 rows).
        Geometry {
            rows: 10_000,
            strip_h: 1000.,
            view_h: 1000.,
            item_h: 20.,
            top,
            max: 10_000. * 20. - 1000.,
        }
    }

    #[test]
    fn a_long_file_scrolls_the_strip_in_proportion() {
        let start = big(0.);
        assert_eq!(start.offset(), 0.);
        assert_eq!(start.frame(), (0., 100.));
        assert_eq!(start.visible_rows(), 0..500);
        let end = big(start.max);
        assert_eq!(end.offset(), 20_000. - 1000.);
        let (top, h) = end.frame();
        assert!(
            (top + h - 1000.).abs() < 0.01,
            "the frame ends at the strip's bottom"
        );
        assert_eq!(end.visible_rows(), 9500..10_000);
        // Half-way: frame in the middle of what is left.
        let mid = big(start.max / 2.);
        assert!((mid.frame().0 - 450.).abs() < 0.01);
    }

    #[test]
    fn a_short_file_does_not_scroll_the_strip() {
        let g = Geometry {
            rows: 100,
            strip_h: 1000.,
            view_h: 500.,
            item_h: 20.,
            top: 600.,
            max: 1500.,
        };
        assert_eq!(g.offset(), 0.);
        assert_eq!(g.frame(), (60., 50.));
        assert_eq!(g.visible_rows(), 0..100);
        // Row 50 (y = 100) centred: 50 * 20 - 250.
        assert_eq!(g.top_centred_at(100.), 750.);
        assert_eq!(g.top_centred_at(0.), 0., "clamped at the top");
        assert_eq!(g.top_centred_at(900.), 1500., "clamped at the end");
        // Dragging the frame 10 px moves the code 10 rows.
        assert_eq!(g.top_dragged(600., 10.), 700.);
    }

    #[test]
    fn dragging_the_frame_keeps_it_under_the_pointer() {
        let g = big(50_000.);
        let (frame0, _) = g.frame();
        let top = g.top_dragged(g.top, 200.);
        let (frame1, _) = big(top).frame();
        assert!(
            (frame1 - frame0 - 200.).abs() < 0.01,
            "{frame0} -> {frame1}"
        );
        assert_eq!(g.top_dragged(g.top, 1e6), g.max);
        assert_eq!(g.top_dragged(g.top, -1e6), 0.);
        assert!(g.on_frame(frame0 + 1.) && !g.on_frame(frame0 - 1.));
    }

    #[test]
    fn nothing_laid_out_is_harmless() {
        let g = Geometry {
            rows: 0,
            strip_h: 0.,
            view_h: 0.,
            item_h: 0.,
            top: 0.,
            max: 0.,
        };
        assert_eq!(g.offset(), 0.);
        assert_eq!(g.visible_rows(), 0..0);
        assert_eq!(g.top_dragged(0., 10.), 0.);
        assert_eq!(g.top_centred_at(5.), 0.);
    }

    #[test]
    fn ticks_in_one_column_on_consecutive_rows_are_one_quad() {
        let tick = |row, col| Tick {
            row,
            col,
            len: 3,
            colour: colour(0.1),
        };
        let ticks: Vec<_> = (0..100).map(|r| tick(r, 4)).chain([tick(50, 20)]).collect();
        let mut ticks = ticks;
        ticks.sort_by_key(|t| (t.row, t.col));
        let mut spans = tick_spans(&ticks);
        spans.sort_by_key(|(t, _)| (t.col, t.row));
        assert_eq!(spans, [(tick(0, 4), 100), (tick(50, 20), 1)]);
        // A gap of one row starts a new quad.
        let spans = tick_spans(&[tick(0, 4), tick(1, 4), tick(3, 4)]);
        assert_eq!(spans, [(tick(0, 4), 2), (tick(3, 4), 1)]);
    }

    #[test]
    fn the_minimap_shows_alone_or_beside_one_other_code_pane() {
        let code = |p: &u32| *p < 10;
        assert_eq!(Focus::of(None, &1, code), Focus::No, "not in focus");
        assert_eq!(Focus::of(Some(&[1]), &1, code), Focus::Alone);
        assert_eq!(Focus::of(Some(&[1, 2]), &2, code), Focus::Pair);
        assert_eq!(
            Focus::of(Some(&[1, 20]), &1, code),
            Focus::No,
            "beside a non-code pane"
        );
        assert_eq!(
            Focus::of(Some(&[1, 2, 3]), &1, code),
            Focus::No,
            "three panes"
        );
        assert_eq!(
            Focus::of(Some(&[2, 3]), &1, code),
            Focus::No,
            "not on the page"
        );
    }

    #[test]
    fn the_shape_is_built_once_per_key() {
        let mm = Minimap::default();
        let key = Key {
            doc: 1,
            epoch: 1,
            folds: 1,
            gutter: 0,
            colours: [0, 0],
        };
        let mut built = 0;
        let mut get = |k: Key| {
            mm.shape(k, || {
                built += 1;
                Shape::build([line("a", &[])], colour(0.5), colour(0.9))
            })
        };
        let a = get(key);
        let b = get(key);
        assert!(Arc::ptr_eq(&a, &b));
        get(Key { folds: 2, ..key });
        get(Key { folds: 2, ..key });
        assert_eq!(built, 2);
    }

    #[test]
    fn a_10k_line_file_is_cheap_to_build_and_hold() {
        let text = "    let value = compute(alpha, beta) + other.field; // comment here";
        let hl = [
            (4..7, style(0.1)),
            (16..23, style(0.3)),
            (52..67, style(0.6)),
        ];
        let t = std::time::Instant::now();
        let shape = Shape::build(
            (0..10_000).map(|_| line(text, &hl)),
            colour(0.5),
            colour(0.9),
        );
        let took = t.elapsed();
        assert_eq!(shape.rows(), 10_000);
        assert!(shape.approx_bytes() < 2_000_000, "{}", shape.approx_bytes());
        // Generous for debug builds on a busy machine; measured numbers are in settings.md.
        assert!(took.as_millis() < 2000, "{took:?}");
    }
}
