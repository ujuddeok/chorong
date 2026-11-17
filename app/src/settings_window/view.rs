//! Drawing the settings window and turning its key presses into model calls.

use super::Shared;
use super::model::{Key, Mode, Model, Outcome, Row, class_label, fmt_duration, measured_text};
use super::remote_model::{RemoteUi, Tab};
use crate::document::hsla;
use cho_layout_ui::assets::icon;
use cho_layout_ui::perf::FrameStats;
use cho_settings::{CostClass, DiagnosticKind, Kind, Source, Value};
use cho_theme::{Rgba, Theme};
use gpui::{
    AnyElement, Context, FocusHandle, Hsla, KeyDownEvent, ScrollHandle, SharedString, Window,
    canvas, deferred, div, prelude::*, px, svg,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

mod remote;

pub struct SettingsWindow {
    theme: Theme,
    shared: Shared,
    model: Model,
    focus: FocusHandle,
    scroll: ScrollHandle,
    ticks: u64,
    frame: Rc<RefCell<FrameStats>>,
    /// The font setting whose family list is open, and the installed families it offers.
    font_menu: Option<&'static str>,
    font_families: Rc<Vec<String>>,
    /// Which section is shown, and the state of Projects, Cache and Accounts.
    tab: Tab,
    remote: RemoteUi,
    /// The font list's and the remote tab's scroll state (`scroll::ScrollAreas`).
    scroll_areas: crate::scroll::ScrollAreas,
    /// The settings list's stretch at its ends.
    over: crate::overscroll::Overscroll,
}

/// `c` at `alpha` (0 to 1).
fn fade(c: Rgba, alpha: f32) -> Hsla {
    hsla(c.with_alpha((alpha * 255.0).round() as u8))
}

impl SettingsWindow {
    /// Lists stretch at their ends when `scroll.overscroll` is on (never in Lightweight mode);
    /// this window has no cost counters, so the stretch is untimed.
    pub(super) fn stretch(&self) -> crate::overscroll::Stretch {
        if self
            .shared
            .store
            .borrow()
            .effective()
            .settings()
            .scroll_overscroll
        {
            crate::overscroll::Stretch::On(None)
        } else {
            crate::overscroll::Stretch::Off
        }
    }

    pub fn new(theme: Theme, shared: Shared, cx: &mut Context<Self>) -> Self {
        // One tick a second: pick up edits made to settings.json outside Chorong, and repaint
        // the measured costs every other tick. (The app-wide owner of the store may take over
        // the reload; calling it here as well is harmless apart from who sees the change.)
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // `CHORONG_FRAME_LOG=FILE` writes this window's frames to `FILE.settings.N` (N counts the
        // windows opened in this run: the logger truncates its file, so each gets its own).
        static OPENED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let log = std::env::var_os("CHORONG_FRAME_LOG").map(|p| {
            let n = OPENED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut s = p;
            s.push(format!(".settings.{n}"));
            std::path::PathBuf::from(s)
        });
        Self {
            theme,
            shared,
            model: Model::default(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            ticks: 0,
            frame: Rc::new(RefCell::new(FrameStats::new(log))),
            font_menu: None,
            font_families: Rc::default(),
            tab: Tab::default(),
            remote: RemoteUi::default(),
            scroll_areas: Default::default(),
            over: Default::default(),
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The window was opened again, possibly from another repository's code window.
    pub fn repo_changed(&mut self, repo: Option<&std::path::Path>, cx: &mut Context<Self>) {
        self.shared.store.borrow_mut().set_active_repo(repo).ok();
        self.model.clamp();
        cx.notify();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.ticks += 1;
        let changed = self
            .shared
            .store
            .borrow_mut()
            .reload_if_changed()
            .is_some_and(|c| !c.is_empty());
        if changed {
            self.model.clamp();
            cx.refresh_windows();
        } else if self.ticks.is_multiple_of(2) {
            cx.notify();
        }
    }

    fn press(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let outcome = self.model.key(&mut self.shared.store.borrow_mut(), key);
        match outcome {
            Outcome::Nothing => {}
            Outcome::Redraw => {
                self.scroll_to_selected();
                cx.notify();
            }
            Outcome::Changed => {
                self.scroll_to_selected();
                // The code window shows a `Lightweight` indicator and reads these values.
                cx.refresh_windows();
            }
            Outcome::Close => window.remove_window(),
        }
    }

    fn scroll_to_selected(&self) {
        // Child 0 of the scrolling column is "Costing the most right now".
        self.scroll.scroll_to_item(self.model.selected + 1);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        // A field with the keys (the search box, a number being typed) gets them first, through
        // the shared field editing, so Cmd-A, copy, paste and word delete work as anywhere else.
        if self.tab == Tab::Settings {
            use cho_layout_ui::field::FieldEdit;
            let paste = (k.key == "v" && k.modifiers.secondary())
                .then(|| cx.read_from_clipboard().and_then(|c| c.text()))
                .flatten();
            match self.model.field(k, paste.as_deref()) {
                FieldEdit::None => {}
                FieldEdit::Copy(text) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                    cx.stop_propagation();
                    return;
                }
                FieldEdit::Text | FieldEdit::Selection => {
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
            }
        }
        let command = k.modifiers.platform || k.modifiers.control;
        if command {
            // Cmd-, is already open: nothing to do.
            if k.key == "w" {
                window.remove_window();
            } else if let Some(tab) = k
                .key
                .chars()
                .next()
                .filter(|_| k.key.len() == 1)
                .and_then(Tab::from_digit)
            {
                self.show_tab(tab, cx);
                cx.stop_propagation();
            } else if self.tab != Tab::Settings && self.remote_typing() {
                // Cmd-A, Cmd-V and Cmd-Backspace in a field on this tab.
                let paste = (k.key == "v")
                    .then(|| cx.read_from_clipboard().and_then(|c| c.text()))
                    .flatten();
                self.remote_key(event, paste, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if self.tab != Tab::Settings {
            self.remote_key(event, None, window, cx);
            cx.stop_propagation();
            return;
        }
        let key = match k.key.as_str() {
            "up" => Key::Up,
            "down" => Key::Down,
            "home" => Key::Home,
            "end" => Key::End,
            "space" => Key::Space,
            "enter" => Key::Enter,
            "escape" => Key::Escape,
            "backspace" => Key::Backspace,
            // The page's own letter keys, by key name (typed text belongs to fields).
            "r" if k.modifiers.shift => Key::Char('R'),
            "r" | "/" if !k.modifiers.shift => Key::Char(k.key.chars().next().unwrap_or('/')),
            _ => {
                // Anything else typed while browsing starts a search with it.
                if let cho_layout_ui::field::FieldKey::Insert(text) =
                    cho_layout_ui::field::field_key(k, None)
                {
                    self.model.type_to_search(&text);
                    cx.notify();
                    cx.stop_propagation();
                }
                return;
            }
        };
        self.press(key, window, cx);
        cx.stop_propagation();
    }

    /// A click on a row's own control: select it, then do what its key does.
    fn click(&mut self, index: usize, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        self.model.select(index);
        self.press(key, window, cx);
        cx.notify();
    }

    /// Sets a font setting to `family` and closes the list.
    fn pick_font(&mut self, key: &'static str, family: &str, cx: &mut Context<Self>) {
        self.font_menu = None;
        let outcome = self
            .model
            .set_text(&mut self.shared.store.borrow_mut(), key, family);
        if outcome == Outcome::Changed {
            cx.refresh_windows();
        } else {
            cx.notify();
        }
    }

    /// Opens or closes a font setting's family list (read from the installed fonts on open).
    fn toggle_font_menu(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if self.font_menu == Some(key) {
            self.font_menu = None;
        } else {
            self.font_families = Rc::new(cho_fonts::families(cx));
            self.font_menu = Some(key);
        }
        cx.notify();
    }

    /// A font setting's picker: the chosen family in its own font, and when open, every
    /// installed family (the bundled default first).
    fn font_picker(
        &self,
        key: &'static str,
        mono: bool,
        value: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (t, ui) = (&self.theme, &self.theme.ui);
        let open = self.font_menu == Some(key);
        let default = if mono {
            cho_fonts::BUNDLED_MONO
        } else {
            cho_fonts::BUNDLED_UI
        };
        let installed = self.font_families.iter().any(|f| f == value) || value == default;
        let button = div()
            .id(SharedString::from(format!("font-pick-{key}")))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py(px(3.))
            .max_w(px(320.))
            .rounded_sm()
            .border_1()
            .border_color(if open {
                hsla(ui.border_focused)
            } else {
                hsla(t.border)
            })
            .bg(hsla(t.editor_background))
            .text_size(px(13.))
            .text_color(hsla(t.text))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_font_menu(key, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(SharedString::from(value.to_string()))
                    .child(value.to_string()),
            )
            .child(div().text_color(hsla(t.text_muted)).child(if open {
                "\u{25b4}"
            } else {
                "\u{25be}"
            }));
        let mut col = div().flex().flex_col().gap_1().child(button);
        if !open && !self.font_families.is_empty() && !installed {
            col = col.child(
                div()
                    .text_size(px(12.))
                    .text_color(hsla(self.warning()))
                    .child(format!("Not installed here: using {default}.")),
            );
        }
        if open {
            let families = self.font_families.clone();
            let entries = std::iter::once(default.to_string())
                .chain(families.iter().filter(|f| f.as_str() != default).cloned())
                .enumerate()
                .map(|(i, family)| {
                    let current = family == value;
                    let label = if i == 0 {
                        format!("{family} (bundled, default)")
                    } else {
                        family.clone()
                    };
                    div()
                        .id(("font-family", i))
                        .px_2()
                        .py(px(2.))
                        .rounded_sm()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .when(current, |d| {
                            d.bg(hsla(t.element_selected)).text_color(hsla(t.text))
                        })
                        .hover(|s| s.bg(hsla(ui.ghost_element_hover)))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.pick_font(key, &family, cx)),
                        )
                        .child(label)
                });
            col = col.child(
                self.scroll_areas
                    .get(format!("font-list-{key}"))
                    .vertical_max(
                        SharedString::from(format!("font-list-{key}")),
                        self.stretch(),
                        px(240.),
                        div()
                            .p_1()
                            .text_size(px(13.))
                            .text_color(hsla(t.text_muted))
                            .children(entries),
                    )
                    .max_w(px(320.))
                    .rounded_sm()
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(ui.elevated_surface_background)),
            );
        }
        col.into_any_element()
    }

    fn toggle_key(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let outcome = self.model.toggle(&mut self.shared.store.borrow_mut(), key);
        if outcome == Outcome::Changed {
            cx.refresh_windows();
        } else {
            cx.notify();
        }
    }

    // ---- Colours ----

    fn warning(&self) -> Rgba {
        self.theme.ui.warning.unwrap_or(self.theme.ui.text_accent)
    }

    fn switch(&self, on: bool, forced: bool) -> gpui::Div {
        let ui = &self.theme.ui;
        let (track, knob) = if on {
            (ui.text_accent, self.theme.editor_background)
        } else {
            (ui.element_background, self.theme.text_muted)
        };
        div()
            .flex_none()
            .w(px(32.))
            .h(px(18.))
            .rounded_full()
            .border_1()
            .border_color(if on {
                hsla(ui.text_accent)
            } else {
                hsla(self.theme.border)
            })
            .bg(hsla(track))
            .when(forced, |d| d.opacity(0.55).border_dashed())
            .flex()
            .items_center()
            .px(px(2.))
            .when(on, |d| d.justify_end())
            .child(div().size(px(12.)).rounded_full().bg(hsla(knob)))
    }

    fn chip(&self, def: &cho_settings::SettingDef) -> gpui::Div {
        let ui = &self.theme.ui;
        let heavy = def.cost_class == CostClass::BackgroundHeavy;
        let (fg, bg) = if heavy {
            let w = self.warning();
            (w, ui.warning_background.map_or_else(|| fade(w, 0.14), hsla))
        } else {
            let fg = match def.cost_class {
                CostClass::PerPointer | CostClass::PerFrame => self.theme.text,
                _ => self.theme.text_muted,
            };
            (fg, hsla(ui.element_background))
        };
        div()
            .flex_none()
            .px(px(6.))
            .py(px(1.))
            .rounded_sm()
            .text_size(px(11.))
            .text_color(hsla(fg))
            .bg(bg)
            .child(class_label(def.cost_class))
    }

    fn badge(&self, source: Source, label: &'static str) -> gpui::Div {
        let (t, ui) = (&self.theme, &self.theme.ui);
        let base = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px(px(6.))
            .py(px(1.))
            .rounded_sm()
            .border_1()
            .text_size(px(11.));
        match source {
            Source::Default => base
                .text_color(hsla(t.text_muted))
                .border_color(hsla(ui.border_variant))
                .child(label),
            Source::Global => base
                .text_color(hsla(t.text))
                .border_color(hsla(t.border))
                .bg(hsla(t.editor_background))
                .child(label),
            Source::Repo => base
                .text_color(hsla(t.text))
                .border_color(fade(ui.text_accent, 0.2))
                .bg(fade(ui.text_accent, 0.2))
                .child(label),
            Source::Lightweight => base
                .text_color(hsla(t.text))
                .border_color(hsla(ui.text_accent))
                .border_dashed()
                .bg(fade(ui.text_accent, 0.12))
                .child(
                    svg()
                        .path(icon("feather"))
                        .size(px(11.))
                        .text_color(hsla(ui.icon_accent)),
                )
                .child(label),
        }
    }

    /// The thin bar showing a feature's share of the time spent in the last five minutes.
    fn cost_bar(&self, row: &Row, share: f32) -> gpui::Div {
        let heavy = row.def.cost_class == CostClass::BackgroundHeavy;
        let fill = if heavy {
            self.warning()
        } else {
            self.theme.ui.text_accent
        };
        div()
            .flex_none()
            .w(px(80.))
            .h(px(4.))
            .rounded_full()
            .bg(hsla(self.theme.ui.element_background))
            .child(
                div()
                    .h_full()
                    .w(gpui::relative(share.clamp(0.0, 1.0)))
                    .rounded_full()
                    .bg(hsla(fill)),
            )
    }

    fn notice(&self, error: bool, text: String) -> gpui::Div {
        let (t, ui) = (&self.theme, &self.theme.ui);
        let (base, bg, border, a_bg, a_border) = if error {
            (ui.error, ui.error_background, ui.error_border, 0.10, 0.50)
        } else {
            (
                ui.warning,
                ui.warning_background,
                ui.warning_border,
                0.09,
                0.45,
            )
        };
        let base = base.unwrap_or(t.text_muted);
        div()
            .flex()
            .gap_2()
            .items_start()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .text_size(px(12.))
            .bg(bg.map_or_else(|| fade(base, a_bg), hsla))
            .border_color(border.map_or_else(|| fade(base, a_border), hsla))
            .child(div().flex_none().text_color(hsla(base)).child(if error {
                "\u{2715}"
            } else {
                "!"
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(hsla(t.text))
                    .child(text),
            )
    }

    // ---- Parts of the page ----

    fn header(&self, repo: Option<String>, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let m = &self.model;
        let filtering = m.mode == Mode::Filter;
        // A search box over the settings: click it (or just type, or press /) to search keys,
        // descriptions and cost hints; Esc or the cross clears it.
        let query = m.filter.clone();
        let count = m.matching();
        let filter: AnyElement = div()
            .id("settings-search")
            .flex()
            .items_center()
            .gap_2()
            .h(px(28.))
            .w(px(320.))
            .px_2()
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(if filtering {
                t.ui.text_accent
            } else {
                t.border
            }))
            .bg(hsla(t.editor_background))
            .cursor_text()
            .on_click(cx.listener(|this, _, _, cx| {
                this.model.start_search();
                cx.notify();
            }))
            .child(
                svg()
                    .path(icon("search"))
                    .size(px(13.))
                    .flex_none()
                    .text_color(hsla(t.text_muted)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(13.))
                    .child(if query.is_empty() && !filtering {
                        div()
                            .text_color(hsla(t.ui.text_disabled))
                            .child("Search settings")
                            .into_any_element()
                    } else if filtering && m.filter_all {
                        // Cmd-A: the whole query selected (typing replaces it).
                        div()
                            .flex()
                            .child(
                                div()
                                    .rounded_sm()
                                    .bg(hsla(t.element_selected))
                                    .text_color(hsla(t.text))
                                    .child(query.clone()),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .text_color(hsla(t.text))
                            .child(format!(
                                "{query}{}",
                                if filtering { "\u{258f}" } else { "" }
                            ))
                            .into_any_element()
                    }),
            )
            .when(!query.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(hsla(t.text_muted))
                        .child(format!(
                            "{count} match{}",
                            if count == 1 { "" } else { "es" }
                        )),
                )
                .child(
                    div()
                        .id("settings-search-clear")
                        .flex_none()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla(t.ui.ghost_element_hover)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.model.clear_search();
                            cx.notify();
                        }))
                        .child(
                            svg()
                                .path(icon("x"))
                                .size(px(12.))
                                .text_color(hsla(t.text_muted)),
                        ),
                )
            })
            .into_any_element();
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .h(px(44.))
            .border_b_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.ui.toolbar_background))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(Tab::ALL.map(|tab| {
                        let on = tab == self.tab;
                        div()
                            .id(("tab", tab as usize))
                            .px_2()
                            .py(px(2.))
                            .rounded_sm()
                            .text_size(px(14.))
                            .cursor_pointer()
                            .text_color(hsla(if on { t.text } else { t.text_muted }))
                            .when(on, |d| d.bg(hsla(t.element_selected)))
                            .hover(|s| s.bg(hsla(t.ui.ghost_element_hover)))
                            .on_click(cx.listener(move |this, _, _, cx| this.show_tab(tab, cx)))
                            .child(tab.label())
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .child(if self.tab == Tab::Settings {
                        filter
                    } else {
                        div().into_any_element()
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(hsla(t.text_muted))
                    .child(match repo {
                        Some(r) => format!("Repository: {r}"),
                        None => "No repository open".to_string(),
                    }),
            )
    }

    fn lightweight_card(
        &self,
        index: usize,
        row: &Row,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (t, ui) = (&self.theme, &self.theme.ui);
        let on = row.is_on();
        let fill = if on {
            crate::markers::blend(ui.text_accent, t.panel_background, 0.08)
        } else {
            t.panel_background
        };
        div()
            .id("lightweight-card")
            .flex()
            .items_center()
            .gap_3()
            .mx_4()
            .mt_3()
            .mb_1()
            .px_4()
            .py_3()
            .rounded_lg()
            .border_1()
            .border_color(if selected {
                hsla(ui.border_focused)
            } else if on {
                fade(ui.text_accent, 0.55)
            } else {
                hsla(t.border)
            })
            .bg(hsla(fill))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.click(index, Key::Space, window, cx);
            }))
            .child(
                svg()
                    .path(icon("feather"))
                    .size(px(22.))
                    .flex_none()
                    .text_color(hsla(if on { ui.icon_accent } else { ui.icon })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(hsla(t.text))
                            .child("Lightweight mode"),
                    )
                    .child(div().text_size(px(12.5)).text_color(hsla(t.text)).child(
                        "Turns every optional feature off at once: a plain, highlighted, \
                         read-only viewer. Turning it off restores each feature's own setting.",
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(t.text_muted))
                            .child(row.def.cost_hint),
                    ),
            )
            .child(self.badge(row.entry.source, row.source_label()))
            .child(self.switch(on, false))
            .into_any_element()
    }

    fn costing_most(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let store = self.shared.store.borrow();
        let top = self.model.costing_most(&store, &self.shared.metrics);
        let mut section = div()
            .flex()
            .flex_col()
            .gap_1()
            .mx_4()
            .mt_2()
            .px_4()
            .py_2()
            .rounded_lg()
            .border_1()
            .border_color(hsla(self.theme.ui.border_variant))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(hsla(t.text_muted))
                    .child("Costing the most right now (last five minutes)"),
            );
        if top.is_empty() {
            section = section.child(
                div()
                    .text_size(px(12.5))
                    .text_color(hsla(t.text_muted))
                    .child("Nothing has taken noticeable time yet."),
            );
        }
        let most = top
            .first()
            .and_then(|r| r.measured)
            .map_or(Duration::ZERO, |m| m.window_total);
        for (i, row) in top.iter().enumerate() {
            let key = row.def.key;
            let total = row.measured.map_or(Duration::ZERO, |m| m.window_total);
            let share = if most.is_zero() {
                0.0
            } else {
                (total.as_secs_f32() / most.as_secs_f32()).max(0.02)
            };
            let toggle = match &row.entry.value {
                Value::Bool(_) => Some(
                    self.switch(row.is_on(), row.entry.source == Source::Lightweight)
                        .id(("top-toggle", i))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_key(key, cx))),
                ),
                Value::UInt(_) | Value::Text(_) => None, // Font: picker added separately
            };
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py(px(2.))
                    .child(
                        div()
                            .w(px(190.))
                            .text_size(px(12.5))
                            .text_color(hsla(t.text))
                            .child(key),
                    )
                    .child(self.cost_bar(row, share))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(hsla(t.text_muted))
                            .child(format!("{} in total", fmt_duration(total))),
                    )
                    .children(toggle),
            );
        }
        section.into_any_element()
    }

    fn setting_row(
        &self,
        index: usize,
        row: &Row,
        selected: bool,
        most: Duration,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (t, ui) = (&self.theme, &self.theme.ui);
        let def = row.def;
        let forced = row.entry.source == Source::Lightweight;
        let bg = if selected {
            hsla(t.element_selected)
        } else if row.repo_scope {
            fade(ui.text_accent, 0.05)
        } else {
            gpui::transparent_black()
        };
        let control: AnyElement = match def.kind {
            Kind::Bool => self
                .switch(row.is_on(), forced)
                .id(("toggle", index))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.click(index, Key::Space, window, cx);
                }))
                .into_any_element(),
            Kind::UInt { .. } => {
                let editing = row.editing.is_some();
                let text = row
                    .editing
                    .clone()
                    .map_or_else(|| row.entry.value.to_string(), |s| format!("{s}\u{258f}"));
                div()
                    .id(("number", index))
                    .flex_none()
                    .min_w(px(64.))
                    .px_2()
                    .py(px(2.))
                    .rounded_sm()
                    .border_1()
                    .border_color(if editing {
                        hsla(ui.border_focused)
                    } else {
                        hsla(t.border)
                    })
                    .bg(hsla(t.editor_background))
                    .text_size(px(13.))
                    .text_color(hsla(t.text))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.click(index, Key::Enter, window, cx);
                    }))
                    .child(text)
                    .into_any_element()
            }
            // A path or version: shown as text, set with `cho settings set` or in the file.
            Kind::Path | Kind::Version => div()
                .id(("text", index))
                .flex_none()
                .max_w(px(72.))
                .overflow_hidden()
                .text_size(px(12.))
                .text_color(hsla(t.text))
                .child(match row.entry.value.to_string() {
                    v if v.is_empty() => "not set".to_string(),
                    v => v,
                })
                .into_any_element(),
            // A choice: the current option as a chip; a click (or Space) steps to the next.
            Kind::Choice { .. } => div()
                .id(("choice", index))
                .flex_none()
                .px_2()
                .py(px(1.))
                .rounded_sm()
                .border_1()
                .border_color(hsla(t.border))
                .bg(hsla(t.editor_background))
                .text_size(px(12.))
                .text_color(hsla(t.text))
                .cursor_pointer()
                .when(forced, |d| d.opacity(0.55).border_dashed())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.click(index, Key::Space, window, cx);
                }))
                .child(choice_label(&row.entry.value.to_string()))
                .into_any_element(),
            // A sample in the chosen font; the picker sits under the description.
            Kind::Font { .. } => div()
                .id(("font", index))
                .flex_none()
                .text_size(px(20.))
                .text_color(hsla(t.text))
                .font_family(SharedString::from(row.entry.value.to_string()))
                .child("Aa")
                .into_any_element(),
        };
        let m = row.measured.as_ref();
        let share = match (m, most.is_zero()) {
            (Some(m), false) => (m.window_total.as_secs_f32() / most.as_secs_f32()).max(0.0),
            _ => 0.0,
        };
        let mono = cx.global::<cho_fonts::Fonts>().mono.clone();
        div()
            .id(("row", index))
            .flex()
            .items_start()
            .gap_3()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(hsla(ui.border_variant))
            .border_l_2()
            .border_color(if selected {
                hsla(ui.border_focused)
            } else {
                hsla(ui.border_variant)
            })
            .bg(bg)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.model.selected = index;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(72.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(control),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                div()
                                    .font_family(mono)
                                    .text_size(px(12.5))
                                    .text_color(hsla(t.text))
                                    .child(def.key),
                            )
                            .child(self.chip(def))
                            .child(self.badge(row.entry.source, row.source_label())),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(hsla(t.text))
                            .child(def.description),
                    )
                    .when_some(
                        match (def.kind, &row.entry.value) {
                            (Kind::Font { mono }, Value::Text(v)) => Some((mono, v.to_string())),
                            _ => None,
                        },
                        |d, (mono, v)| d.child(self.font_picker(def.key, mono, &v, cx)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(hsla(t.text_muted))
                            .child(def.cost_hint),
                    )
                    .when(forced, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(hsla(ui.text_accent))
                                .child(row.entry.explanation()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_size(px(12.))
                            .text_color(hsla(t.text_muted))
                            .child(self.cost_bar(row, share))
                            .child(measured_text(def, m)),
                    ),
            )
            .child(
                div()
                    .id(("scope", index))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(hsla(t.text_muted))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.click(index, Key::Char('r'), window, cx);
                    }))
                    .child(self.switch(row.repo_scope, false))
                    .child("this repository only"),
            )
            .into_any_element()
    }

    fn footer(&self) -> impl IntoElement {
        let t = &self.theme;
        let key = |k: &'static str| {
            div()
                .px(px(5.))
                .rounded_sm()
                .bg(hsla(t.ui.element_background))
                .text_color(hsla(t.text))
                .child(k)
        };
        let item = |k: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(key(k))
                .child(what)
        };
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_4()
            .gap_y_1()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.status_bar_background))
            .text_size(px(12.))
            .text_color(hsla(t.text_muted))
            .when(self.tab != Tab::Settings, |d| {
                d.child(self.remote_footer_hint())
            })
            .when(self.tab == Tab::Settings, |d| {
                d.child(item("\u{2191}\u{2193}", "move"))
                    .child(item("Space", "toggle"))
                    .child(item("Enter", "edit a number"))
                    .child(item("R", "this repository only"))
                    .child(item("\u{232b}", "reset"))
                    .child(item("/", "filter"))
                    .child(item("Esc", "close"))
            })
    }

    /// The frame timer's last step: runs once everything else has painted.
    fn frame_end(&self) -> AnyElement {
        let stats = self.frame.clone();
        deferred(
            canvas(|_, _, _| {}, move |_, _, _, _| stats.borrow_mut().end())
                .absolute()
                .size_0(),
        )
        .priority(usize::MAX)
        .into_any_element()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame.borrow_mut().begin();
        let (rows, repo, diagnostics, version) = {
            let store = self.shared.store.borrow();
            let rows = self.model.rows(&store, &self.shared.metrics);
            let repo = store.active_repo().map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .map_or_else(|| p.to_string(), |n| n.to_string_lossy().into_owned())
            });
            (rows, repo, store.diagnostics(), store.version())
        };
        let _ = version;
        self.model.clamp();
        let most = rows
            .iter()
            .filter_map(|r| r.measured.map(|m| m.window_total))
            .max()
            .unwrap_or(Duration::ZERO);

        let mut notices: Vec<AnyElement> = Vec::new();
        if let Some(n) = &self.model.notice {
            notices.push(self.notice(true, n.clone()).into_any_element());
        }
        let total = diagnostics.len();
        for d in diagnostics.iter().take(4) {
            let error = matches!(
                d.kind,
                DiagnosticKind::Malformed | DiagnosticKind::Unreadable
            );
            notices.push(self.notice(error, d.to_string()).into_any_element());
        }
        if total > 4 {
            notices.push(
                self.notice(
                    false,
                    format!("and {} more in the settings files", total - 4),
                )
                .into_any_element(),
            );
        }

        let mut list = div().flex().flex_col().child(self.costing_most(cx));
        if rows.is_empty() {
            list = list.child(
                div()
                    .p_6()
                    .text_color(hsla(self.theme.text_muted))
                    .child("No setting matches. Esc clears the filter."),
            );
        }
        for (i, row) in rows.iter().enumerate() {
            let selected = i == self.model.selected;
            list = list.child(if row.is_lightweight_card() {
                self.lightweight_card(i, row, selected, cx)
            } else {
                self.setting_row(i, row, selected, most, cx)
            });
        }

        let root = div()
            .key_context("ChorongSettings")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .font_family(cx.global::<cho_fonts::Fonts>().ui.clone())
            .bg(hsla(self.theme.background))
            .text_color(hsla(self.theme.text))
            .text_size(px(13.))
            .child(self.header(repo, cx))
            .when(!notices.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .px_4()
                        .pt_3()
                        .children(notices),
                )
            })
            .child(if self.tab == Tab::Settings {
                crate::scroll::ScrollArea::with(self.scroll.clone(), self.over.clone())
                    .vertical("settings-rows", self.stretch(), list)
                    .flex_1()
                    .min_h_0()
                    .into_any_element()
            } else {
                self.remote_body(cx)
            })
            .child(self.footer())
            .child(self.frame_end());
        self.frame.borrow_mut().built();
        root
    }
}

/// A choice option as the settings page shows it: the word, capitalised.
fn choice_label(option: &str) -> String {
    let mut c = option.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}
