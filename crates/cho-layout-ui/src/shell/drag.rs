//! Drag and drop: what is dragged, where a drop would land, and applying it.

use super::*;

/// What is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dragged {
    /// One pane, by its tab or a tray chip.
    Pane(PaneId),
    /// A whole group, by its header grip.
    Group(NodeId),
}

#[derive(Clone)]
pub struct DragPayload {
    pub(super) what: Dragged,
    pub(super) title: SharedString,
    pub(super) icon: &'static str,
    pub(super) colors: Colors,
}

/// The drag preview: a small chip just below and right of the pointer, so it never hides the
/// drop zone or accent line under the pointer. GPUI draws the preview at the pointer minus
/// where the drag started inside the source element, hence the offset.
pub(super) struct DragChip(pub(super) DragPayload, pub(super) Point<Pixels>);

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let c = &self.0.colors;
        div()
            .pl(self.1.x + px(14.))
            .pt(self.1.y + px(16.))
            .font_family(c.ui_font.clone())
            .child(
                div()
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(4.))
                    .bg(c.elevated)
                    .border_1()
                    .border_color(c.text_accent)
                    .shadow_md()
                    .text_size(px(12.))
                    .text_color(c.text)
                    .child(small_icon(self.0.icon, c.icon))
                    .child(self.0.title.clone()),
            )
    }
}

/// Resize drags carry what they resize; their preview is empty.
#[derive(Clone, Copy)]
pub(super) struct SplitHandle {
    pub(super) split: NodeId,
    pub(super) handle: usize,
    pub(super) axis: Axis,
}

#[derive(Clone, Copy)]
pub(super) struct DockHandle(pub(super) DockId);

pub(super) struct Nothing;

impl Render for Nothing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Where a drop would land, as shown while dragging.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Hint {
    /// Beside a group (an edge) or onto it as a tab (the centre).
    Zone {
        group: NodeId,
        edge: Option<Edge>,
    },
    /// Between tabs: the accent line. `from` is the tab-strip slot whose drag listener set
    /// it, so only that slot clears it (neighbours share a boundary index).
    Tab {
        group: NodeId,
        index: usize,
        from: usize,
    },
    Dock(DockId),
    /// Along the whole of one edge of the code area (or the focus page).
    AreaEdge(Edge),
    Tray,
    /// Onto a page's chip in the focus bar: the pane moves to that page.
    Page(usize),
    /// Onto the current focus page while it is empty.
    EmptyPage,
}

impl Hint {
    /// The model target, for hints that are one (not a page chip or the empty page).
    fn target(self) -> Option<Target> {
        Some(match self {
            Hint::Zone {
                group,
                edge: Some(edge),
            } => Target::Beside { group, edge },
            Hint::Zone { group, edge: None } => Target::Tabs {
                group,
                index: usize::MAX,
            },
            Hint::Tab { group, index, .. } => Target::Tabs { group, index },
            Hint::Dock(d) => Target::Dock(d),
            Hint::AreaEdge(e) => Target::AreaEdge(e),
            Hint::Tray => Target::Tray,
            Hint::Page(_) | Hint::EmptyPage => return None,
        })
    }
}

/// Applies a drop to `ws`: all or nothing, and one undo step.
pub(super) fn try_apply(ws: &mut Workspace, what: Dragged, hint: Hint) -> cho_layout::Result<()> {
    let panes = match what {
        Dragged::Pane(p) => vec![p],
        Dragged::Group(g) => ws
            .group(g)
            .map(|g| g.tabs.clone())
            .ok_or(cho_layout::LayoutError::UnknownGroup(g))?,
    };
    ws.batch(|w| match (hint.target(), hint) {
        (Some(t), _) => match what {
            Dragged::Pane(p) => w.move_pane(p, t),
            Dragged::Group(g) => w.move_group(g, t),
        },
        (None, Hint::Page(i)) => panes.iter().try_for_each(|p| w.move_pane_to_page(*p, i)),
        (None, _) => {
            w.place_on_empty_page(panes[0])?;
            let group = w.group_of(panes[0]).expect("just placed");
            panes[1..].iter().try_for_each(|p| {
                w.move_pane(
                    *p,
                    Target::Tabs {
                        group,
                        index: usize::MAX,
                    },
                )
            })
        }
    })
}

impl Shell {
    pub(super) fn set_hint(&mut self, what: Dragged, hint: Hint, cx: &mut Context<Self>) {
        // Drag moves arrive at the pointer's rate; probe the model only when the target changes.
        if self.dragged == Some(what) && self.hint == Some(hint) {
            return;
        }
        self.dragged = Some(what);
        // One pane to a target: the model's rules answer without a copy. Groups and pages are
        // tried on a copy.
        let ok = match (what, hint.target()) {
            (Dragged::Pane(p), Some(t)) => self.ws.validate(p, t).is_ok(),
            _ => try_apply(&mut self.ws.clone(), what, hint).is_ok(),
        };
        let hint = ok.then_some(hint);
        if self.hint != hint {
            self.hint = hint;
            cx.notify();
        }
    }

    pub(super) fn clear_hint_if(&mut self, f: impl Fn(Hint) -> bool, cx: &mut Context<Self>) {
        if self.hint.is_some_and(f) {
            self.hint = None;
            cx.notify();
        }
    }

    pub(super) fn drop_payload(&mut self, payload: &DragPayload, cx: &mut Context<Self>) {
        if let Some(hint) = self.hint.take() {
            self.apply(cx, |ws| try_apply(ws, payload.what, hint));
        }
        cx.notify();
    }

    pub(super) fn payload(&self, what: Dragged) -> DragPayload {
        let pane = match what {
            Dragged::Pane(p) => Some(p),
            Dragged::Group(g) => self.ws.group(g).and_then(Group::active_pane),
        };
        let content = pane.and_then(|p| self.ws.content(p));
        DragPayload {
            what,
            title: content.map_or("".into(), |c| c.title().into()),
            icon: content.map_or("square-dashed", |c| kind_icon(c.kind())),
            colors: self.c.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_layout::PageOrigin;

    fn editor(path: &str) -> PaneContent {
        PaneContent::Editor {
            path: path.into(),
            caret: 0,
        }
    }

    /// A focus layout whose first page holds one group of three editor tabs, with an empty
    /// second page. Returns the workspace, the group and its panes.
    fn three_tabs_and_an_empty_page() -> (Workspace, NodeId, Vec<PaneId>) {
        let mut ws = Workspace::with_defaults();
        let panes: Vec<PaneId> = ["a.rs", "b.rs", "c.rs"]
            .into_iter()
            .map(|p| ws.open(editor(p), None).unwrap())
            .collect();
        ws.maximize(panes[0], 0).unwrap();
        let group = ws.group_of(panes[0]).unwrap();
        for p in &panes[1..] {
            let t = Target::Tabs {
                group,
                index: usize::MAX,
            };
            ws.move_pane(*p, t).unwrap();
        }
        ws.new_page(Some(PageOrigin::default())).unwrap();
        ws.go_to_page(0).unwrap();
        ws.clear_undo();
        (ws, group, panes)
    }

    /// One drop, one undo step: undoing it once gives back the layout from before it.
    fn assert_one_step(ws: &mut Workspace, before: &Workspace) {
        assert_eq!(ws.undo_stack().len(), 1);
        ws.undo().unwrap();
        assert_eq!(ws.focus(), before.focus());
        assert_eq!(ws.arrangement(), before.arrangement());
    }

    #[test]
    fn dropping_a_group_on_a_page_chip_is_one_undo_step() {
        let (mut ws, group, panes) = three_tabs_and_an_empty_page();
        let before = ws.clone();
        try_apply(&mut ws, Dragged::Group(group), Hint::Page(1)).unwrap();
        let f = ws.focus().unwrap();
        assert!(panes.iter().all(|p| f.page_of(*p) == Some(1)));
        assert_one_step(&mut ws, &before);
    }

    #[test]
    fn dropping_on_the_empty_page_is_one_undo_step() {
        let (mut ws, _, panes) = three_tabs_and_an_empty_page();
        ws.go_to_page(1).unwrap();
        let before = ws.clone();
        try_apply(&mut ws, Dragged::Pane(panes[1]), Hint::EmptyPage).unwrap();
        assert_eq!(ws.focus().unwrap().page_of(panes[1]), Some(1));
        assert_one_step(&mut ws, &before);
    }

    #[test]
    fn a_refused_drop_changes_nothing() {
        let (mut ws, group, _) = three_tabs_and_an_empty_page();
        let before = ws.clone();
        assert!(try_apply(&mut ws, Dragged::Group(group), Hint::Page(7)).is_err());
        assert_eq!(ws.focus(), before.focus());
        assert!(ws.undo_stack().is_empty());
    }

    #[test]
    fn page_hints_are_not_model_targets() {
        assert_eq!(Hint::Page(0).target(), None);
        assert_eq!(Hint::EmptyPage.target(), None);
        assert_eq!(Hint::Tray.target(), Some(Target::Tray));
    }
}
