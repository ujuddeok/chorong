//! The layout shell: renders a `cho-layout` workspace and turns mouse and keyboard input into
//! its operations. All layout decisions are the model's; this file only draws and hit-tests.
//!
//! The host (the viewer or the lab) owns the window frame. Its root element tracks the
//! host's focus handle and carries the host's key context, calls [`bind_actions`] to attach the
//! layout actions, holds the [`Shell`] entity as a child, and draws the status bar from
//! [`Shell::status_left`], [`Shell::hint_bar`] and [`Shell::notice`]. Pane contents come from
//! the host through [`PaneBodies`].

mod actions;
mod chrome;
mod drag;
mod layout_mode;
mod pickers;
mod render;
mod stats;

pub use actions::bind_actions;
pub use drag::{DragPayload, Dragged};
pub use pickers::TrayTabMode;

use drag::{DockHandle, DragChip, Hint, Nothing, SplitHandle};
use render::{key_chip, tip, tip_owned};

use crate::actions::*;
use crate::colors::Colors;
use crate::keys;
use crate::perf::{self, FrameStats, Saver};
use crate::widgets::{kind_icon, small_icon};
use cho_layout::{
    Axis, Direction, DockId, Edge, Group, LABEL_LETTERS, Label, Node, NodeId, PaneContent, PaneId,
    PaneKind, Pin, PinPlace, Rect, ResizeStep, Target, Workspace,
};
use cho_metrics::{FeatureId, Metrics};
use cho_theme::Theme;
use gpui::{
    Action, AnyElement, AnyView, App, AppContext, Context, DragMoveEvent, Entity, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point, Render,
    SharedString, StatefulInteractiveElement, StyleRefinement, Styled, WeakEntity, Window,
    anchored, canvas, deferred, div, prelude::FluentBuilder, px, relative,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// A header chip supplied by the host: its text and what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    pub tone: BadgeTone,
}

/// How a [`Badge`] is tinted, from the theme's status colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeTone {
    /// The accent colour.
    Neutral,
    /// Success (green-ish): something added.
    Good,
    /// Error (red-ish): something removed.
    Bad,
}

/// How the host draws a focus page's chip (the viewer: a tour step's badge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipLook {
    /// Outlined in `success`.
    Done,
    /// Filled `text.accent`.
    Current,
    /// Outlined in `border`.
    ToDo,
    /// Outlined in `error`.
    Broken,
    /// Dashed `border`: a page the reader added.
    Own,
}

/// A focus page's chip as the host wants it drawn, in place of the plain page number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageChip {
    pub label: SharedString,
    pub look: ChipLook,
    /// A dot on the chip: the reader rearranged the page.
    pub edited: bool,
    /// A divider before the chip (the first of the reader's own pages after a tour's).
    pub divider_before: bool,
    pub tooltip: Option<SharedString>,
}

/// What the host supplies for each pane: its body and the short muted text after its title.
pub trait PaneBodies: 'static {
    /// The pane's contents, drawn inside its group below the header.
    fn body(
        &self,
        pane: PaneId,
        content: &PaneContent,
        ws: &Workspace,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;

    /// Host state outside the pane's content and size that changes how its body looks (the
    /// viewer: whether it is the first editor). The shell redraws a cached body when this
    /// changes; the default means "nothing".
    fn body_key(&self, _pane: PaneId, _content: &PaneContent, _ws: &Workspace) -> u64 {
        0
    }

    /// The text after the pane's title in its header (counts, status).
    fn qualifier(&self, pane: PaneId, content: &PaneContent, cx: &App) -> String;

    /// Whether the qualifier is a warning (a pin whose file changed since it was taken): it is
    /// then drawn in `warning` instead of muted text. The default means "never".
    fn qualifier_warns(&self, _pane: PaneId, _content: &PaneContent, _cx: &App) -> bool {
        false
    }

    /// An optional small chip after the qualifier in the pane's header (the viewer: which side
    /// of a comparison an editor shows). The default means "none".
    fn badge(&self, _pane: PaneId, _content: &PaneContent, _cx: &App) -> Option<Badge> {
        None
    }

    /// Buttons the host adds to the pane's header, before minimise and maximise (the viewer:
    /// Pin and ↓ on the Context pane). Drawn for the group's active tab. The default means
    /// "none". Headers are cached views, redrawn when the qualifier, badge or layout change
    /// (`Shell::header_key`), so these buttons must not look different for any other reason.
    fn header_actions(
        &self,
        _pane: PaneId,
        _content: &PaneContent,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }

    /// Host state the header's [`PaneBodies::header_actions`] show (the viewer: whether a
    /// code pane is half of a linked split), hashed. A cached header is drawn anew when it
    /// changes; the default means "nothing".
    fn header_key(&self, _pane: PaneId, _content: &PaneContent, _cx: &App) -> u64 {
        0
    }

    /// Entries the host adds to the pane menu (⋯ or a right click), each a label and the
    /// action it dispatches on the pane, listed before "Split right" (the viewer: Expand All
    /// and Collapse All on the Changes pane). The default means "none".
    fn menu_items(
        &self,
        _pane: PaneId,
        _content: &PaneContent,
        _cx: &App,
    ) -> Vec<(SharedString, Box<dyn Action>)> {
        Vec::new()
    }

    /// The context the "pin context" actions pin, for the `n`th pin made, or `None` when there
    /// is nothing to pin (the shell then says so in the status bar). The lab invents
    /// placeholders; the viewer pins the symbol under the caret or pointer.
    fn pin(&self, n: usize, cx: &App) -> Option<Pin>;

    /// How focus page `index`'s chip looks, or `None` for the plain number (the default).
    /// Must not read the shell entity: it is called while the shell draws.
    fn page_chip(&self, _index: usize, _ws: &Workspace, _cx: &App) -> Option<PageChip> {
        None
    }

    /// Something the host draws to the left of the focus page, outside its arrangement (the
    /// viewer: the tour rail). `None` by default. Must not read the shell entity.
    fn focus_side(
        &self,
        _ws: &Workspace,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }

    /// What Tab switches "Add to page from the tray…" to: the word in its footer and hint, and
    /// the words whose start switches (see [`TrayTabMode`]). Tab then asks for the host's
    /// picker with `AddFileToPage`. The default is Chorong's `"files"`; a host with another
    /// picker names it, as `TrayTabMode::new("issues").with_alias("jira")`. Must not read the
    /// shell entity: it is called while the shell draws.
    fn tray_tab_mode(&self) -> TrayTabMode {
        TrayTabMode::default()
    }
}

/// One pane's body as a view of its own, so GPUI can reuse its layout and paint while the
/// shell redraws around it (dragging, resizing, Layout mode). GPUI redraws it when its
/// bounds change or it is notified; the shell notifies it through [`Shell::refresh_pane`].
struct PaneBody {
    pane: PaneId,
    shell: WeakEntity<Shell>,
    bodies: Rc<dyn PaneBodies>,
}

impl Render for PaneBody {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shell) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let (pane, bodies) = (self.pane, self.bodies.clone());
        // A view is laid out as a root, so the body gets the same block parent that the
        // shell's container used to give it.
        let body = shell.update(cx, |s, cx| match s.ws.content(pane) {
            Some(content) => bodies.body(pane, &content.clone(), &s.ws, window, cx),
            None => div().into_any_element(),
        });
        div().size_full().child(body).into_any_element()
    }
}

/// A pane's body view and what it was built for: when either changes the shell makes a new
/// view (a new view has no cached frame).
struct BodySlot {
    view: Entity<PaneBody>,
    content: PaneContent,
    key: u64,
}

/// One group's header as a view of its own, so a frame that redraws the shell (a pane body
/// changed, the pointer moved) reuses the headers whose inputs did not change. The shell makes
/// a new view when [`Shell::header_key`] changes; GPUI redraws it when its bounds change.
struct PaneHeader {
    group: NodeId,
    shell: WeakEntity<Shell>,
}

impl Render for PaneHeader {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shell) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let group = self.group;
        let header = shell.update(cx, |s, cx| match s.ws.group(group).cloned() {
            Some(g) => s.render_header(&g, cx),
            None => div().into_any_element(),
        });
        // A view is laid out as a root: the header fills the box the group gave it.
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .into_any_element()
    }
}

/// A header's view and the key it was built for.
struct HeaderSlot {
    view: Entity<PaneHeader>,
    key: u64,
}

const HEADER: f32 = 28.;
const HANDLE: f32 = 5.;
/// How close to a pane's edge (as a share of its size) a drop splits instead of adding a tab.
const EDGE_SHARE: f32 = 0.25;

struct Menu {
    pane: PaneId,
    at: Point<Pixels>,
}

/// What floats over the layout. At most one is open: opening one closes the others.
enum Popup {
    None,
    /// The pane menu (⋯ or a right click).
    Menu(Menu),
    /// Reopen focus layout….
    History,
    /// A pane picker or the Ctrl-Tab switcher; it takes the focus.
    Overlay(pickers::Overlay),
}

/// The shell view: everything of the layout UI except the status bar.
pub struct Shell {
    ws: Workspace,
    c: Colors,
    /// Bumped whenever `c` changes, so cached headers are drawn anew.
    colors_generation: u64,
    bodies: Rc<dyn PaneBodies>,
    /// One cached view per pane that has been drawn, dropped when the pane leaves the workspace.
    pane_bodies: RefCell<HashMap<PaneId, BodySlot>>,
    /// One cached view per group header that has been drawn, dropped with its group.
    headers: RefCell<HashMap<NodeId, HeaderSlot>>,
    /// The host's focus handle, which the shell hands focus back to after leaving Layout mode
    /// and after clicks.
    return_focus: FocusHandle,
    hint: Option<Hint>,
    /// What the current drag carries, learnt from its move events (GPUI only hands the dragged
    /// value to drag listeners).
    dragged: Option<Dragged>,
    popup: Popup,
    notice: Option<String>,
    pins_made: usize,
    /// Frame timing, shared with the paint callback that ends each frame.
    stats: Rc<RefCell<FrameStats>>,
    show_stats: bool,
    /// Save the layout on change (`layout.autosave`); the saver thread exists either way.
    autosave: bool,
    /// Cost of the overlay and the saver hand-off, with the ids for them.
    metrics: Option<(Arc<Metrics>, FeatureId, FeatureId)>,
    saver: Option<Saver>,
    /// The workspace changed since the saver last got a copy.
    dirty: bool,
    /// Each group's size in the last frame, measured at prepaint. Used only to skip drawing
    /// what cannot be seen (tabs past the strip's end, bodies of squeezed panes); it never
    /// changes the layout itself.
    sizes: Rc<RefCell<HashMap<NodeId, gpui::Bounds<Pixels>>>>,
    /// Layout mode (design/layout-keyboard.md), while it is on.
    mode: Option<Mode>,
    /// The hint bar's focus: entering the mode focuses it, so its `LayoutMode` key context
    /// wins over every pane's keys.
    mode_focus: FocusHandle,
    /// Where each arrow would move the pane (or its group) while Shift (or Alt) is held in
    /// Layout mode, worked out at the start of each frame.
    previews: Vec<(Direction, Target)>,
    /// A move happened while previewing: draw once more after this frame, whose measured
    /// group sizes the next preview needs.
    preview_stale: bool,
    /// Focused while a [`Popup::Overlay`] is open.
    overlay_focus: FocusHandle,
}

/// Layout mode's state: the resize sub-mode, and jump labels fixed when the mode opened (and
/// refreshed after a layout change).
struct Mode {
    resize: bool,
    /// Shift (one pane) or Alt (its group) is held: preview where the arrows would move it.
    preview: Option<PreviewOf>,
    labels: Vec<(Label, NodeId)>,
    /// Labels on the tray's chips in a focus layout, carrying on from `labels`: pressing one
    /// brings that pane up in the focused spot.
    tray: Vec<(Label, PaneId)>,
}

impl Mode {
    /// Recomputes the jump labels for what is on screen and in the tray.
    fn relabel(&mut self, ws: &Workspace) {
        self.labels = ws.labels(&LABEL_LETTERS);
        self.tray = ws.tray_labels(&LABEL_LETTERS, &self.labels);
    }
}

/// What Layout mode's arrow preview moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewOf {
    /// The pane (Shift held).
    Pane,
    /// Its whole group (Alt held).
    Group,
}

/// Shell options, from the host's command line or settings.
pub struct ShellOptions {
    /// Write one line per frame to this file.
    pub frame_log: Option<PathBuf>,
    /// Save the layout under this directory (off the UI thread, debounced).
    pub state_dir: Option<PathBuf>,
    /// The repository key the saver files the layout under (the lab uses `"layout-lab"`).
    pub state_key: String,
    /// Start with the frame-time overlay shown.
    pub show_stats: bool,
    /// Save the layout when it changes (needs `state_dir`).
    pub autosave: bool,
    /// Where to report frame budgets and the cost of `debug.frame_overlay`, `debug.frame_log`
    /// and `layout.autosave`. Needs those three keys registered.
    pub metrics: Option<Arc<Metrics>>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Shell {
    pub fn new(
        ws: Workspace,
        theme: &Theme,
        options: ShellOptions,
        bodies: Rc<dyn PaneBodies>,
        return_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let ids = options.metrics.as_ref().and_then(|m| {
            Some((
                m.clone(),
                m.id("debug.frame_overlay")?,
                m.id("layout.autosave")?,
                m.id("debug.frame_log")?,
            ))
        });
        let mut stats = FrameStats::new(options.frame_log);
        if let Some((m, _, _, log)) = &ids {
            stats = stats.with_metrics(m.clone(), *log);
        }
        Self {
            stats: Rc::new(RefCell::new(stats)),
            show_stats: options.show_stats,
            autosave: options.autosave,
            metrics: ids.map(|(m, overlay, autosave, _)| (m, overlay, autosave)),
            saver: options
                .state_dir
                .map(|d| Saver::spawn(d, options.state_key)),
            dirty: false,
            sizes: Rc::default(),
            mode: None,
            mode_focus: cx.focus_handle(),
            previews: Vec::new(),
            preview_stale: false,
            overlay_focus: cx.focus_handle(),
            ws,
            c: Colors::new(theme, crate::fonts::pick(cx)),
            colors_generation: 0,
            bodies,
            pane_bodies: RefCell::default(),
            headers: RefCell::default(),
            return_focus,
            hint: None,
            dragged: None,
            popup: Popup::None,
            notice: None,
            pins_made: 0,
        }
    }

    /// Applies settings that changed while running: the overlay, layout saving and the frame
    /// log (`frame_log` is the file to write, or `None` for no log).
    pub fn apply_options(
        &mut self,
        show_stats: bool,
        autosave: bool,
        frame_log: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.stats.borrow_mut().set_log(frame_log);
        if self.show_stats != show_stats || self.autosave != autosave {
            self.show_stats = show_stats;
            self.autosave = autosave;
            cx.notify();
        }
    }

    /// Uses `fonts` from now on (the UI font setting changed): headers, tabs, menus and the
    /// shell's own text are drawn anew with it.
    pub fn set_fonts(&mut self, fonts: crate::fonts::Fonts, cx: &mut Context<Self>) {
        if self.c.ui_font == fonts.ui && self.c.mono_font == fonts.mono {
            return;
        }
        self.c.ui_font = fonts.ui;
        self.c.mono_font = fonts.mono;
        self.colors_generation += 1;
        cx.notify();
    }

    /// The workspace, for the host's own status fields.
    pub fn workspace(&self) -> &Workspace {
        &self.ws
    }

    /// Redraws one pane's body: the host's state for it changed.
    pub fn refresh_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(slot) = self.pane_bodies.borrow().get(&pane) {
            slot.view.update(cx, |_, cx| cx.notify());
        }
    }

    /// Redraws every pane body: the host's state changed in a way not tied to one pane.
    pub fn refresh_all_panes(&mut self, cx: &mut Context<Self>) {
        for slot in self.pane_bodies.borrow().values() {
            slot.view.update(cx, |_, cx| cx.notify());
        }
    }

    /// The cached view of `pane`'s body, made anew when its content or host key changed.
    fn body_view(&self, pane: PaneId, content: &PaneContent, cx: &mut Context<Self>) -> AnyView {
        let key = self.bodies.body_key(pane, content, &self.ws);
        let mut map = self.pane_bodies.borrow_mut();
        let fresh = |cx: &mut Context<Self>| {
            let (shell, bodies) = (cx.weak_entity(), self.bodies.clone());
            let view = cx.new(|_| PaneBody {
                pane,
                shell,
                bodies,
            });
            // Drawn once more on the next frame. GPUI's cached view remembers the entities its
            // render touched for the first time in the frame, and `cx.new` here has already
            // touched the new view, so its first cached state leaves the view itself out. A
            // frame that then reuses it unregisters the view from the window, and every later
            // `refresh_pane` (a graph laid out, an index answer) is lost until something else
            // redraws it. A second render, in a frame that did not create it, records it.
            let weak = view.downgrade();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            });
            view
        };
        let slot = match map.entry(pane) {
            std::collections::hash_map::Entry::Occupied(o) => {
                let slot = o.into_mut();
                if slot.key != key || slot.content != *content {
                    *slot = BodySlot {
                        view: fresh(cx),
                        content: content.clone(),
                        key,
                    };
                }
                slot
            }
            std::collections::hash_map::Entry::Vacant(v) => v.insert(BodySlot {
                view: fresh(cx),
                content: content.clone(),
                key,
            }),
        };
        let style = StyleRefinement::default().size_full();
        AnyView::from(slot.view.clone()).cached(style)
    }

    /// The shell's resolved colours, for the host's chrome.
    pub fn colors(&self) -> &Colors {
        &self.c
    }

    /// The last refused operation, for the status bar.
    pub fn notice(&self) -> Option<String> {
        self.notice.clone()
    }

    /// Whether Esc has something to close in the shell: an open menu or history list, or a
    /// focus layout to restore. A host whose own Esc binding wins hands over when this is true
    /// by dispatching `RestoreLayout`.
    pub fn escape_pending(&self) -> bool {
        matches!(self.popup, Popup::Menu(_) | Popup::History) || self.ws.in_focus()
    }

    /// Opens `popup` in place of whatever was open. An overlay takes the focus (leaving
    /// Layout mode); closing an overlay gives the focus back to the host.
    fn open_popup(&mut self, popup: Popup, window: &mut Window, cx: &mut Context<Self>) {
        let opens_overlay = matches!(popup, Popup::Overlay(_));
        if opens_overlay && self.mode.is_some() {
            self.exit_mode(window, cx);
        }
        let old = std::mem::replace(&mut self.popup, popup);
        if opens_overlay {
            window.focus(&self.overlay_focus);
        } else if matches!(old, Popup::Overlay(_)) {
            window.focus(&self.return_focus);
        }
        cx.notify();
    }

    /// Closes whatever is open; false when nothing was.
    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if matches!(self.popup, Popup::None) {
            return false;
        }
        self.open_popup(Popup::None, window, cx);
        true
    }

    fn overlay(&self) -> Option<&pickers::Overlay> {
        match &self.popup {
            Popup::Overlay(o) => Some(o),
            _ => None,
        }
    }

    fn overlay_mut(&mut self) -> Option<&mut pickers::Overlay> {
        match &mut self.popup {
            Popup::Overlay(o) => Some(o),
            _ => None,
        }
    }

    /// Takes the overlay, when one is open (the caller hands the focus back).
    fn take_overlay(&mut self) -> Option<pickers::Overlay> {
        match std::mem::replace(&mut self.popup, Popup::None) {
            Popup::Overlay(o) => Some(o),
            other => {
                self.popup = other;
                None
            }
        }
    }

    /// Whether Layout mode is on.
    pub fn in_layout_mode(&self) -> bool {
        self.mode.is_some()
    }

    /// Runs a model operation for the host (marks the layout changed and redraws). Unlike the
    /// shell's own operations, a refusal is returned, not shown as a notice.
    pub fn update_workspace<T>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace) -> cho_layout::Result<T>,
    ) -> cho_layout::Result<T> {
        let started = std::time::Instant::now();
        let t = f(&mut self.ws);
        self.stats.borrow_mut().last_op_us = started.elapsed().as_secs_f32() * 1e6;
        debug_assert!(self.ws.check_invariants().is_ok());
        self.dirty = true;
        if let Some(m) = &mut self.mode {
            m.relabel(&self.ws);
        }
        cx.notify();
        t
    }

    // ----- applying operations ---------------------------------------------------------------

    /// Runs a model operation, turning a refusal into a status-bar notice.
    fn apply<T>(
        &mut self,
        cx: &mut Context<Self>,
        op: impl FnOnce(&mut Workspace) -> cho_layout::Result<T>,
    ) {
        let r = self.update_workspace(cx, op);
        self.notice = r.err().map(|e| e.to_string());
    }

    /// The next pin, or (with a notice) `None` when the host has nothing to pin.
    fn next_pin(&mut self, cx: &mut Context<Self>) -> Option<Pin> {
        let pin = self.bodies.pin(self.pins_made, cx);
        if pin.is_some() {
            self.pins_made += 1;
        } else {
            self.notice = Some("Nothing to pin: put the caret on a symbol first".into());
            cx.notify();
        }
        pin
    }

    /// The pane actions act on: the focused one, else the first on screen.
    fn target_pane(&self) -> Option<PaneId> {
        self.ws
            .focused()
            .filter(|p| self.ws.group_of(*p).is_some())
            .or_else(|| match self.ws.focus() {
                Some(f) => f.on_screen().first().copied(),
                None => self.ws.arrangement().panes().first().copied(),
            })
    }

    fn on_pane(
        &mut self,
        cx: &mut Context<Self>,
        op: impl FnOnce(&mut Workspace, PaneId) -> cho_layout::Result<()>,
    ) {
        if let Some(p) = self.target_pane() {
            self.apply(cx, |ws| op(ws, p));
        }
    }
}
