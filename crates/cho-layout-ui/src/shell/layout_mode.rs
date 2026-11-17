//! Layout mode: entering and leaving it, keyboard moves and the hint bar.

use super::*;
use gpui::ModifiersChangedEvent;

impl Shell {
    // ----- Layout mode ------------------------------------------------------------------------

    pub(super) fn enter_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = Popup::None;
        let mut mode = Mode {
            resize: false,
            preview: None,
            labels: Vec::new(),
            tray: Vec::new(),
        };
        mode.relabel(&self.ws);
        self.mode = Some(mode);
        window.focus(&self.mode_focus);
        cx.notify();
    }

    /// Tab / Shift-Tab in Layout mode: the next (or previous) file in the tray takes the
    /// focused spot, and the file there goes back to the tray.
    pub(super) fn flip_tray(&mut self, forward: bool, cx: &mut Context<Self>) {
        match self.ws.tray_file(forward) {
            Some(p) => self.apply(cx, |ws| ws.swap_from_tray(p)),
            None => {
                self.notice = Some(if self.ws.in_focus() {
                    "No files in the tray".into()
                } else {
                    "Flipping through files works in a focus layout".into()
                });
                cx.notify();
            }
        }
    }

    /// A jump label pressed in Layout mode that names a tray chip: that pane takes the
    /// focused spot (the mode stays open, labels refreshed). False when no chip has it.
    pub(super) fn bring_tray_label(&mut self, label: &str, cx: &mut Context<Self>) -> bool {
        let pane = self.mode.as_ref().and_then(|m| {
            m.tray
                .iter()
                .find(|(l, _)| l.to_string() == label)
                .map(|(_, p)| *p)
        });
        match pane {
            Some(p) => {
                self.apply(cx, |ws| ws.swap_from_tray(p));
                true
            }
            None => false,
        }
    }

    pub(super) fn exit_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = None;
        self.ws.seal_undo();
        window.focus(&self.return_focus);
        cx.notify();
    }

    /// Group rectangles from the last frame, in reading order, for neighbour queries.
    pub(super) fn rects(&self) -> Vec<(NodeId, Rect)> {
        let measured = self.sizes.borrow();
        self.ws
            .screen_group_ids()
            .into_iter()
            .filter_map(|id| {
                measured.get(&id).map(|b| {
                    (
                        id,
                        Rect::new(
                            f32::from(b.origin.x),
                            f32::from(b.origin.y),
                            f32::from(b.size.width),
                            f32::from(b.size.height),
                        ),
                    )
                })
            })
            .collect()
    }

    pub(super) fn focus_dir(&mut self, dir: Direction, cx: &mut Context<Self>) {
        let rects = self.rects();
        self.apply(cx, |ws| ws.focus_dir(dir, &rects).map(|_| ()));
    }

    pub(super) fn move_dir(&mut self, whole_group: bool, dir: Direction, cx: &mut Context<Self>) {
        self.preview_stale = true;
        let rects = self.rects();
        if whole_group {
            let group = self.target_pane().and_then(|p| self.ws.group_of(p));
            if let Some(g) = group {
                self.apply(cx, |ws| ws.move_group_dir(g, dir, &rects).map(|_| ()));
            }
        } else {
            self.on_pane(cx, |ws, p| ws.move_pane_dir(p, dir, &rects).map(|_| ()));
        }
    }

    /// Where each arrow would move the pane, or its group, while Shift or Alt is held in
    /// Layout mode (not in its resize sub-mode).
    pub(super) fn arrow_previews(&self) -> Vec<(Direction, Target)> {
        let Some(of) = self
            .mode
            .as_ref()
            .filter(|m| !m.resize)
            .and_then(|m| m.preview)
        else {
            return Vec::new();
        };
        match self.target_pane() {
            Some(p) => self
                .ws
                .dir_targets(p, of == PreviewOf::Group, &self.rects()),
            None => Vec::new(),
        }
    }

    /// The Layout-mode hint bar for the status bar, when the mode is on.
    pub fn hint_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.mode
            .is_some()
            .then(|| self.render_hint_bar(window, cx))
    }

    /// The mode's hint bar, in place of the status bar's right half: every key read from the
    /// keymap in the mode's context, so a remapped key shows here too.
    pub(super) fn render_hint_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let resize = self.mode.as_ref().is_some_and(|m| m.resize);
        let ctx = if resize {
            "LayoutMode resize"
        } else {
            "LayoutMode"
        };
        let key = |a: &dyn Action| keys::label_in(a, window, ctx);
        let arrows = |acts: [&dyn Action; 4]| keys::arrows(acts.map(key));
        let resize_dir = |d: Dir, fine: bool| ResizePane { direction: d, fine };
        let items: Vec<(Option<String>, &str)> = if resize {
            vec![
                (
                    arrows([
                        &resize_dir(Dir::Left, false),
                        &resize_dir(Dir::Up, false),
                        &resize_dir(Dir::Down, false),
                        &resize_dir(Dir::Right, false),
                    ]),
                    "move edge",
                ),
                (
                    arrows([
                        &resize_dir(Dir::Left, true),
                        &resize_dir(Dir::Up, true),
                        &resize_dir(Dir::Down, true),
                        &resize_dir(Dir::Right, true),
                    ]),
                    "fine",
                ),
                (key(&EqualizePanes), "equalise"),
                (key(&ExitResizeMode), "done"),
            ]
        } else {
            vec![
                (
                    arrows([
                        &FocusPaneLeft,
                        &FocusPaneUp,
                        &FocusPaneDown,
                        &FocusPaneRight,
                    ]),
                    "go to",
                ),
                (
                    arrows([&MovePaneLeft, &MovePaneUp, &MovePaneDown, &MovePaneRight]),
                    "move",
                ),
                (
                    arrows([
                        &MoveGroupLeft,
                        &MoveGroupUp,
                        &MoveGroupDown,
                        &MoveGroupRight,
                    ]),
                    "group",
                ),
                (key(&EnterResizeMode), "resize"),
                (
                    key(&SplitPaneRight)
                        .zip(key(&SplitPaneDown))
                        .map(|(a, b)| format!("{a}/{b}")),
                    "split",
                ),
                (key(&EqualizePanes), "equalise"),
                (key(&ToggleMaximizePane), "max"),
                (key(&MinimizePane), "min"),
                (
                    key(&UndoLayout)
                        .zip(key(&RedoLayout))
                        .map(|(a, b)| format!("{a}/{b}")),
                    "undo",
                ),
                (Some("1–9 a d f…".into()), "jump"),
                (key(&ExitLayoutMode), "leave"),
            ]
        };
        // In a focus layout with files in the tray: Tab flips the focused spot through them.
        let mut items = items;
        if !resize && self.ws.tray_file(true).is_some() {
            items.insert(
                0,
                (
                    key(&NextTrayFile)
                        .zip(key(&PrevTrayFile))
                        .map(|(a, b)| format!("{a}/{b}")),
                    "next file",
                ),
            );
        }
        div()
            .id("layout-mode")
            .track_focus(&self.mode_focus)
            .key_context(ctx)
            // Holding Shift (or Alt) lights up where each arrow would move the pane (or group).
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| {
                let m = e.modifiers;
                let preview = match (m.shift, m.alt, m.control || m.platform) {
                    (true, false, false) => Some(PreviewOf::Pane),
                    (false, true, false) => Some(PreviewOf::Group),
                    _ => None,
                };
                if let Some(mode) = &mut this.mode
                    && mode.preview != preview
                {
                    mode.preview = preview;
                    cx.notify();
                }
            }))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .h_full()
            .flex()
            .items_center()
            .gap(px(12.))
            .pl(px(10.))
            .border_l_1()
            .border_color(c.border)
            .child(
                div()
                    .text_color(c.text_accent)
                    .font_weight(FontWeight::MEDIUM)
                    .child(if resize { "Resize" } else { "Layout" }),
            )
            .children(items.into_iter().filter_map(|(k, what)| {
                k.map(|k| {
                    div()
                        .flex_none()
                        .flex()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_color(c.text)
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(k)),
                        )
                        .child(SharedString::from(what.to_string()))
                })
            }))
            .into_any_element()
    }
}
