//! The file tree pane (the left sidebar): folders and files from `crate::tree`, styled per
//! docs/public/visual-design.md ("File tree", "Row states", "Icons").
//!
//! The tree's own state (expanded folders, visible rows, scroll) lives in a GPUI global keyed
//! by the `Chorong` view, not on `Chorong`, so the pane keeps to its own module.

use super::shell_host::OpenAt;
use super::*;
use crate::tree::{Row, RowKind, Tree, active_guide, file_icon, reveal};
use cho_layout_ui::widgets::small_icon;
use gpui::{EntityId, Global, ScrollStrategy};
use std::path::Path;
use std::rc::Rc;

/// Row height, inset from the pane edge, indent per level and the chevron slot.
const ROW_H: f32 = 22.;
const INSET: f32 = 4.;
const PAD: f32 = 4.;
const INDENT: f32 = 12.;
const CHEVRON: f32 = 14.;

/// One window's tree: built once from the file list, rows rebuilt when a folder opens or
/// closes or another file is revealed (never per frame).
struct TreeState {
    tree: Tree,
    expanded: HashSet<PathBuf>,
    rows: Rc<Vec<Row>>,
    /// The open file's parent-folder rows and depth, for the active indent guide.
    guide: Option<(std::ops::Range<usize>, usize)>,
    /// The file last revealed (relative), so a reveal happens once per file opened.
    revealed: Option<PathBuf>,
    dirty: bool,
    scroll: UniformListScrollHandle,
    over: crate::overscroll::Overscroll,
}

impl TreeState {
    fn new(files: &[PathBuf]) -> Self {
        Self {
            tree: Tree::new(files),
            expanded: HashSet::new(),
            rows: Rc::default(),
            guide: None,
            revealed: None,
            dirty: true,
            scroll: UniformListScrollHandle::new(),
            over: Default::default(),
        }
    }

    fn rebuild(&mut self) {
        self.rows = Rc::new(self.tree.rows(&self.expanded));
        self.guide = self
            .revealed
            .as_deref()
            .and_then(|cur| active_guide(&self.rows, cur));
        self.dirty = false;
    }
}

#[derive(Default)]
struct TreeStates(HashMap<EntityId, TreeState>);

impl Global for TreeStates {}

impl Chorong {
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if let Some(search) = &self.search {
            // Grown to the window (Cmd-Shift-F again): the results, and the selected one's
            // file beside them (`picker.preview`).
            if self.search_full_screen() && self.features.on(Feature::PickerPreview) {
                return div()
                    .size_full()
                    .flex()
                    .child(
                        div()
                            .w(gpui::relative(0.45))
                            .flex_none()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(self.render_search(search, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .children(self.render_peek()),
                    )
                    .into_any_element();
            }
            return self.render_search(search, cx).into_any_element();
        }
        // An open tour shows only its files (the Tour files chip turns that off).
        if let Some(tour) = self.tour_tree() {
            return self.render_tour_tree(tour, cx);
        }
        self.render_file_list(cx).into_any_element()
    }

    /// The open file relative to the root, if it is inside it.
    fn tree_current(&self) -> Option<PathBuf> {
        let doc = self.ed().doc.as_ref()?;
        let root = self.root.as_ref()?;
        doc.path.strip_prefix(root).ok().map(Path::to_path_buf)
    }

    pub(super) fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.tree_current();
        let id = cx.entity_id();
        let (rows, guide, scroll, over) = {
            let states = cx.default_global::<TreeStates>();
            let st = states
                .0
                .entry(id)
                .or_insert_with(|| TreeState::new(&self.files));
            // Reveal: opening a file from anywhere expands its ancestors and scrolls it into
            // view, once per file.
            let reveal_now = current.is_some() && st.revealed != current;
            if reveal_now && let Some(cur) = &current {
                reveal(cur, &mut st.expanded);
                st.revealed = current.clone();
                st.dirty = true;
            }
            if st.dirty {
                st.rebuild();
            }
            if reveal_now
                && let Some(ix) = st
                    .rows
                    .iter()
                    .position(|r| Some(&r.rel) == current.as_ref())
            {
                st.scroll.scroll_to_item(ix, ScrollStrategy::Center);
            }
            (
                st.rows.clone(),
                st.guide.clone(),
                st.scroll.clone(),
                st.over.clone(),
            )
        };
        let t = &self.theme;
        let c = crate::pane_style::colors(&self.theme, cx);
        let (guide_col, guide_active) = (
            hsla(t.ui.panel_indent_guide),
            hsla(t.ui.panel_indent_guide_active),
        );
        let probe = self.features.probe_if_on(Feature::Overscroll);
        div()
            .size_full()
            .pt_1()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .child(
                crate::scroll::uniform_list(
                    "files",
                    rows.len(),
                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|ix| {
                                let row = &rows[ix];
                                let selected = current.as_ref() == Some(&row.rel);
                                let active_level = guide
                                    .as_ref()
                                    .filter(|(r, _)| r.contains(&ix))
                                    .map(|(_, d)| *d);
                                tree_row(
                                    ix,
                                    row,
                                    selected,
                                    active_level,
                                    &c,
                                    guide_col,
                                    guide_active,
                                    cx,
                                )
                            })
                            .collect()
                    }),
                )
                .wrap(&over, &scroll, probe)
                .flex_1()
                .min_h_0(),
            )
    }
}

/// Toggles a folder in this window's tree.
fn toggle_folder(rel: &Path, cx: &mut Context<Chorong>) {
    let id = cx.entity_id();
    if let Some(st) = cx.default_global::<TreeStates>().0.get_mut(&id) {
        if !st.expanded.remove(rel) {
            st.expanded.insert(rel.to_path_buf());
        }
        st.dirty = true;
    }
    cx.notify();
}

/// One tree row: chevron slot, icon, name (compact chains as `a / b`), indent guides.
#[allow(clippy::too_many_arguments)]
fn tree_row(
    ix: usize,
    row: &Row,
    selected: bool,
    active_level: Option<usize>,
    c: &cho_layout_ui::colors::Colors,
    guide_col: gpui::Hsla,
    guide_active: gpui::Hsla,
    cx: &mut Context<Chorong>,
) -> gpui::AnyElement {
    let left = PAD + INDENT * row.depth as f32;
    // Guides: one per ancestor level, through the chevron centre of that level.
    let guides = (0..row.depth).map(|level| {
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(PAD + INDENT * level as f32 + CHEVRON / 2.))
            .w(px(1.))
            .bg(if active_level == Some(level) {
                guide_active
            } else {
                guide_col
            })
    });
    let (chevron, icon, icon_col) = match row.kind {
        RowKind::Folder { expanded } => (
            Some(if expanded {
                "chevron-down"
            } else {
                "chevron-right"
            }),
            if expanded { "folder-open" } else { "folder" },
            c.icon,
        ),
        RowKind::File => (
            None,
            file_icon(row.segments.last().map_or("", String::as_str)),
            if selected { c.text } else { c.icon },
        ),
    };
    let mut label = div()
        .flex()
        .min_w_0()
        .overflow_hidden()
        .when(selected, |d| d.font_weight(FontWeight::MEDIUM));
    for (i, seg) in row.segments.iter().enumerate() {
        if i > 0 {
            label = label.child(div().px_1().text_color(c.text_muted).child("/"));
        }
        label = label.child(seg.clone());
    }
    let rel = row.rel.clone();
    let is_folder = matches!(row.kind, RowKind::Folder { .. });
    let row_el = div()
        .id(("tree", ix))
        .relative()
        .w_full()
        .h(px(ROW_H))
        .rounded(px(4.))
        .flex()
        .items_center()
        .pl(px(left))
        .pr_2()
        .whitespace_nowrap()
        .overflow_hidden()
        .cursor_pointer()
        .when(selected, |d| d.bg(c.selected))
        .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
        .active(|s| s.bg(c.active))
        .children(guides)
        .child(
            div()
                .flex_none()
                .w(px(CHEVRON))
                .mr(px(4.))
                .flex()
                .justify_center()
                .children(chevron.map(|ch| small_icon(ch, c.icon))),
        )
        .child(div().mr(px(6.)).flex().child(small_icon(icon, icon_col)))
        .child(label)
        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
            if is_folder {
                toggle_folder(&rel, cx);
            } else if let Some(root) = this.root.clone() {
                // Alt-click opens to the right, Alt-Shift-click below.
                this.open_at(root.join(&rel), OpenAt::of_click(&e.modifiers()), cx);
            }
        }))
        // Right-click on a file: Show graph for it.
        .when(!is_folder, |d| {
            let rel = row.rel.clone();
            d.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    let path = rel.to_string_lossy().replace('\\', "/");
                    this.show_graph(Some(crate::graph::Scope::File { path }), cx);
                }),
            )
        });
    // The row is inset from the pane edge, so its fill has rounded ends.
    div()
        .w_full()
        .px(px(INSET))
        .child(row_el)
        .into_any_element()
}
